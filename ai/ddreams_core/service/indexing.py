from __future__ import annotations

import concurrent.futures
from collections import deque
from contextlib import closing
import hashlib
import json
import os
import sqlite3
import time
from pathlib import Path
from typing import Any
import numpy as np

from ..constants import (
    DIMENSION,
    COLOR_VERSION,
    QUALITY_VERSION,
    DECODER_VERSION,
    IMAGE_RETRY_EXTENSIONS,
    RAW_EXTENSIONS,
    RAW_RETRY_ERRORS,
    FACE_VERSION,
    BATCH_SIZE,
    DECODE_THREADS,
    COLOR_PALETTE,
)
from ..colors import palette_counts, dominant_colors, learn_thresholds
from ..util import RequestError, _set_background_priority
from ..quality import compute_quality
from ..vector_index import VectorIndex


class IndexingMixin:
    def _retry_decode_failures(self) -> None:
        if self.faces_db is None:
            return
        # Startup only, before either worker starts. Retain the AI failure rows
        # until the face transaction commits, so an interrupted migration can resume.
        with self.db:
            self.db.execute("BEGIN IMMEDIATE")
            marker = self.db.execute("SELECT value FROM meta WHERE key='raw_decoder_version'").fetchone()
            version = int(marker[0]) if marker is not None else 0
            if version >= DECODER_VERSION:
                return

            path_extensions = IMAGE_RETRY_EXTENSIONS | (RAW_EXTENSIONS if version < 1 else frozenset())
            retry_ids, other_ids = [], []
            for asset_id, error in self.db.execute("SELECT asset_id,error FROM failures"):
                kind = error.partition(":")[0]
                if kind == "LibRawTooBigError":
                    continue
                (retry_ids if version < 1 and kind in RAW_RETRY_ERRORS else other_ids).append(asset_id)

            # The app puts ai.sqlite beside the catalogue and inherits DDREAMS_CATALOG
            # when an alternate catalogue filename is configured.
            catalog = Path(os.environ.get("DDREAMS_CATALOG") or Path(self.db_path).with_name("catalog.sqlite"))
            if other_ids and catalog.is_file():
                with closing(sqlite3.connect(catalog.resolve().as_uri() + "?mode=ro", uri=True)) as conn:
                    for start in range(0, len(other_ids), 500):
                        chunk = other_ids[start:start + 500]
                        marks = ",".join("?" for _ in chunk)
                        for asset_id, path in conn.execute(f"SELECT id,path FROM assets WHERE id IN ({marks})", chunk):
                            if path and Path(path).suffix.lower().lstrip(".") in path_extensions:
                                retry_ids.append(asset_id)

            with self.faces_db:
                self.faces_db.executemany(
                    """DELETE FROM face_scan WHERE asset_id=?
                       AND NOT EXISTS (SELECT 1 FROM faces WHERE faces.asset_id=face_scan.asset_id)""",
                    ((asset_id,) for asset_id in retry_ids),
                )
            self.db.executemany("DELETE FROM failures WHERE asset_id=?", ((asset_id,) for asset_id in retry_ids))
            self.db.execute("INSERT OR REPLACE INTO meta(key,value) VALUES('raw_decoder_version',?)",
                            (str(DECODER_VERSION),))

    def _validate_items(self, raw: Any) -> list[dict[str, Any]]:
        if not isinstance(raw, list):
            raise RequestError("items musí byť zoznam")
        result, seen = [], set()
        for item in raw:
            if not isinstance(item, dict):
                raise RequestError("Každá položka musí byť objekt")
            aid, path, size, mtime = item.get("asset_id"), item.get("path"), item.get("size"), item.get("mtime_ns")
            if isinstance(aid, bool) or not isinstance(aid, int) or aid <= 0:
                raise RequestError("asset_id musí byť kladné celé číslo")
            if aid in seen:
                raise RequestError("items obsahuje duplicitné asset_id")
            if not isinstance(path, str) or not path:
                raise RequestError("path musí byť neprázdny reťazec")
            if any(isinstance(v, bool) or not isinstance(v, int) or v < 0 for v in (size, mtime)):
                raise RequestError("size a mtime_ns musia byť nezáporné celé čísla")
            seen.add(aid)
            result.append({"asset_id": aid, "path": path, "size": size, "mtime_ns": mtime})
        return result

    def _index_command(self, request: dict[str, Any]) -> dict[str, Any]:
        items = self._validate_items(request.get("items"))
        prune = request.get("prune", False)
        append = request.get("append", False)
        if not isinstance(prune, bool):
            raise RequestError("prune musí byť true alebo false")
        if not isinstance(append, bool):
            raise RequestError("append musí byť true alebo false")
        if append and prune:
            raise RequestError("append a prune nemožno použiť súčasne")
        ids = {item["asset_id"] for item in items}
        if not append:
            with self._condition:
                self._generation += 1
                generation = self._generation
                self._queue = []
                self._condition.notify_all()
        with self.db_lock:
            removed = 0
            if prune:
                current = {int(row[0]) for table in ("embeddings", "failures", "colors", "quality")
                           for row in self.db.execute(f"SELECT asset_id FROM {table}")}
                remove = current - ids
                removed = len(remove)
                self._delete_index_rows(remove)
                self.indexed_count = int(self.db.execute("SELECT count(*) FROM embeddings WHERE model=?", (self.model_name,)).fetchone()[0])
            old = {int(a): (str(m), int(s), int(t)) for a, m, s, t in self.db.execute("SELECT asset_id,model,size,mtime_ns FROM embeddings")}
            failed_old = {int(a): (int(s), int(t)) for a, s, t in self.db.execute("SELECT asset_id,size,mtime_ns FROM failures")}
            color_old = {int(a): (int(s), int(t), int(v)) for a, s, t, v in self.db.execute("SELECT asset_id,size,mtime_ns,version FROM colors")}
            quality_old = {int(a): (s, t, v) for a, s, t, v in self.db.execute("SELECT asset_id,size,mtime_ns,version FROM quality")}
            face_old = {}
            if self.faces_db is not None:
                with self.faces_lock:
                    face_old = {int(a): (s, t, v) for a, s, t, v in self.faces_db.execute("SELECT asset_id,size,mtime_ns,version FROM face_scan")}
            queued, upto, failed_before = [], 0, 0
            quality_queued = []
            face_queued = []
            for item in items:
                aid, size, mtime = item["asset_id"], item["size"], item["mtime_ns"]
                if face_old.get(aid) != (size, mtime, FACE_VERSION):
                    face_queued.append({**item, "face_scan": True})
                if old.get(aid) == (self.model_name, size, mtime):
                    upto += 1
                    if color_old.get(aid) != (size, mtime, COLOR_VERSION):
                        queued.append({**item, "colors_only": True})
                    elif quality_old.get(aid) != (size, mtime, QUALITY_VERSION):
                        quality_queued.append({**item, "quality_only": True})
                elif failed_old.get(aid) == (size, mtime):
                    failed_before += 1
                else:
                    queued.append({**item, "colors_only": False})
                    if aid in failed_old:
                        self.db.execute("DELETE FROM failures WHERE asset_id=?", (aid,))
            queued.sort(key=lambda item: hashlib.sha256(str(item["asset_id"]).encode("ascii")).digest())
            self.db.commit()
            if removed and self._vectors is not None:
                self._vectors.remove(remove)
        if prune and self.faces_db is not None:
            with self.faces_lock:
                known_faces = {int(row[0]) for row in self.faces_db.execute("SELECT asset_id FROM face_scan UNION SELECT DISTINCT asset_id FROM faces")}
                drop_ids = list(known_faces - ids)
                for start in range(0, len(drop_ids), 500):
                    chunk = drop_ids[start:start + 500]
                    marks = ",".join("?" for _ in chunk)
                    fids = [int(row[0]) for row in self.faces_db.execute(f"SELECT id FROM faces WHERE asset_id IN ({marks})", chunk)] if chunk else []
                    if fids:
                        fmarks = ",".join("?" for _ in fids)
                        self.faces_db.execute(f"DELETE FROM face_thumbs WHERE face_id IN ({fmarks})", fids)
                        self.faces_db.execute(f"DELETE FROM face_rejected WHERE face_id IN ({fmarks})", fids)
                        self.faces_db.execute(f"DELETE FROM faces WHERE id IN ({fmarks})", fids)
                    if chunk:
                        self.faces_db.execute(f"DELETE FROM face_scan WHERE asset_id IN ({marks})", chunk)
                self.faces_db.commit()
                self._refresh_face_references_locked()
        if face_queued and self.faces_db is not None:
            # Photos that already carry imported (named) faces go first: they become the references.
            with self.faces_lock:
                with_faces = {int(row[0]) for row in self.faces_db.execute("SELECT DISTINCT asset_id FROM faces WHERE vector IS NULL")}
            face_queued.sort(key=lambda item: item["asset_id"] not in with_faces)
        with self._condition:
            pending_faces = {item["asset_id"] for item in self._face_scan_queue}
            pending_faces |= self._face_active_ids
            if append:
                active_faces = pending_faces
                self._face_scan_queue.extend(item for item in face_queued if item["asset_id"] not in active_faces)
            else:
                self._face_scan_queue = deque(face_queued)
            queued_ids = {item["asset_id"] for item in queued}
            self._quality_queue = [item for item in self._quality_queue if item["asset_id"] not in queued_ids]
            if append:
                queued_ids = {item["asset_id"] for item in self._queue}
                queued = [item for item in queued if item["asset_id"] not in queued_ids]
                self._queue.extend(queued)
                priority_ids = queued_ids | {item["asset_id"] for item in queued}
                pending_quality = {item["asset_id"] for item in self._quality_queue}
                quality_to_add = [item for item in quality_queued if item["asset_id"] not in priority_ids | pending_quality]
                self._quality_queue.extend(quality_to_add)
                self._total += len(queued) + len(quality_to_add)
                if queued and self._started_at is None:
                    self._started_at = time.monotonic()
            else:
                self._generation = generation
                self._queue = queued
                self._quality_queue = [item for item in quality_queued if item["asset_id"] not in queued_ids]
                self._done = self._failed = self._offline = 0
                self._total = len(queued) + len(self._quality_queue)
                self._started_at = time.monotonic() if queued else None
                self._last_error = None
            self._condition.notify_all()
        self._emit({"event": "status", **self.status()})
        return {"queued": len(queued), "up_to_date": upto, "failed_before": failed_before, "pruned": removed}

    def _forget_command(self, request: dict[str, Any]) -> dict[str, Any]:
        """Drop assets that left the catalogue (excluded folder): stored rows, search index and queue."""
        asset_ids = request.get("asset_ids")
        if not isinstance(asset_ids, list) or any(isinstance(a, bool) or not isinstance(a, int) for a in asset_ids):
            raise RequestError("asset_ids musí byť zoznam ID")
        forget = set(asset_ids)
        if not forget:
            return {"forgotten": 0}
        with self.db_lock:
            known = set()
            for start in range(0, len(asset_ids), 500):
                chunk = asset_ids[start:start + 500]
                marks = ",".join("?" for _ in chunk)
                known.update(int(row[0]) for row in self.db.execute(f"SELECT asset_id FROM embeddings WHERE asset_id IN ({marks})", chunk))
                for table in ("embeddings", "failures", "colors", "quality"):
                    self.db.execute(f"DELETE FROM {table} WHERE asset_id IN ({marks})", chunk)
            self.db.commit()
            self.indexed_count = int(self.db.execute("SELECT count(*) FROM embeddings WHERE model=?", (self.model_name,)).fetchone()[0])
            if self._vectors is not None:
                self._vectors.remove(forget)
        if self.faces_db is not None:
            with self.faces_lock:
                for start in range(0, len(asset_ids), 500):
                    chunk = asset_ids[start:start + 500]
                    if not chunk: continue
                    marks = ",".join("?" for _ in chunk)
                    face_ids = [int(row[0]) for row in self.faces_db.execute(f"SELECT id FROM faces WHERE asset_id IN ({marks})", chunk)]
                    if face_ids:
                        fmarks = ",".join("?" for _ in face_ids)
                        self.faces_db.execute(f"DELETE FROM face_thumbs WHERE face_id IN ({fmarks})", face_ids)
                        self.faces_db.execute(f"DELETE FROM face_rejected WHERE face_id IN ({fmarks})", face_ids)
                        self.faces_db.execute(f"DELETE FROM faces WHERE id IN ({fmarks})", face_ids)
                    self.faces_db.execute(f"DELETE FROM face_scan WHERE asset_id IN ({marks})", chunk)
                self.faces_db.commit()
                self._refresh_face_references_locked()
        with self._condition:
            before = len(self._queue)
            self._queue = [item for item in self._queue if item["asset_id"] not in forget]
            before_quality = len(self._quality_queue)
            self._quality_queue = [item for item in self._quality_queue if item["asset_id"] not in forget]
            self._face_scan_queue = deque(item for item in self._face_scan_queue if item["asset_id"] not in forget)
            self._total = max(0, self._total - (before - len(self._queue)) - (before_quality - len(self._quality_queue)))
            self._condition.notify_all()
        return {"forgotten": len(known)}

    def _delete_index_rows(self, ids: set[int]) -> None:
        ids = list(ids)
        for start in range(0, len(ids), 500):
            chunk = ids[start:start + 500]
            marks = ",".join("?" for _ in chunk)
            for table in ("embeddings", "failures", "colors", "quality"):
                self.db.execute(f"DELETE FROM {table} WHERE asset_id IN ({marks})", chunk)

    def _light_sync(self, request: dict[str, Any]) -> dict[str, Any]:
        asset_ids = request.get("asset_ids")
        if not isinstance(asset_ids, list) or not asset_ids or any(isinstance(a, bool) or not isinstance(a, int) or a <= 0 for a in asset_ids):
            raise RequestError("asset_ids musí byť neprázdny zoznam kladných ID")
        listed = set(asset_ids)
        with self.db_lock:
            embedded = {int(r[0]) for r in self.db.execute("SELECT asset_id FROM embeddings WHERE model=? AND dim=?", (self.model_name, DIMENSION))}
            failures = {int(r[0]) for r in self.db.execute("SELECT asset_id FROM failures")}
            colors = {int(r[0]) for r in self.db.execute("SELECT asset_id FROM colors WHERE version=?", (COLOR_VERSION,))}
            quality = {int(r[0]) for r in self.db.execute("SELECT asset_id FROM quality WHERE version=?", (QUALITY_VERSION,))}
            known = set()
            for table in ("embeddings", "failures", "colors", "quality"):
                known.update(int(r[0]) for r in self.db.execute(f"SELECT asset_id FROM {table}"))
        scanned = set()
        if self.faces_db is not None:
            with self.faces_lock:
                scanned = {int(r[0]) for r in self.faces_db.execute("SELECT asset_id FROM face_scan WHERE version=?", (FACE_VERSION,))}
        vanished = known - listed
        if vanished:
            with self.db_lock:
                self._delete_index_rows(vanished)
                self.db.commit()
                self.indexed_count = int(self.db.execute("SELECT count(*) FROM embeddings WHERE model=?", (self.model_name,)).fetchone()[0])
                if self._vectors is not None:
                    self._vectors.remove(vanished)
            with self._condition:
                self._queue = [item for item in self._queue if item["asset_id"] not in vanished]
                self._quality_queue = [item for item in self._quality_queue if item["asset_id"] not in vanished]
                self._face_scan_queue = deque(item for item in self._face_scan_queue if item["asset_id"] not in vanished)
        needs_vectors = listed - embedded - failures
        needs_metadata = listed - (colors & quality)
        needs_faces = listed - scanned if self.faces_db is not None else set()
        return {"asset_ids": sorted(needs_vectors | needs_metadata | needs_faces), "pruned": len(vanished)}

    def _index_loop(self) -> None:
        _set_background_priority()
        pool = concurrent.futures.ThreadPoolExecutor(max_workers=DECODE_THREADS, thread_name_prefix="ai-decode", initializer=_set_background_priority)
        try:
            while True:
                with self._condition:
                    while not self._stop and (self._paused or not self._model_ready.is_set() or self._model_error or (not self._queue and not self._quality_queue)):
                        self._condition.wait(timeout=0.5)
                    if self._stop:
                        return
                    generation = self._generation
                    if self._queue:
                        batch, self._queue = self._queue[:BATCH_SIZE], self._queue[BATCH_SIZE:]
                    else:
                        batch, self._quality_queue = self._quality_queue[:4], self._quality_queue[4:]
                    self._active = True
                self._process_batch(batch, generation, pool)
        finally:
            pool.shutdown(wait=True)

    def _emit_indexed_assets(self) -> None:
        with self._condition:
            if (not self._newly_indexed_ids or self._active or self._queue or self._quality_queue
                    or self._face_scan_queue or self._face_active_ids):
                return
            ids = sorted(self._newly_indexed_ids)
            self._newly_indexed_ids.clear()
        self._emit({"event": "indexed_assets", "asset_ids": ids})

    def _process_batch(self, batch: list[dict[str, Any]], generation: int, pool: concurrent.futures.ThreadPoolExecutor) -> None:
        success, errors, offline, color_rows, quality_rows = [], [], [], [], []
        futures = {pool.submit(self.decoder, item): item for item in batch if os.path.isfile(item["path"])}
        offline.extend(item for item in batch if not os.path.isfile(item["path"]))
        decoded, decoded_items = [], []
        for future, item in futures.items():
            try:
                image = future.result()
                decoded.append(image)
                decoded_items.append(item)
                quality_rows.append((item, getattr(image, "info", {}).get("ddreams_quality") or compute_quality(image)))
                if not item.get("quality_only"):
                    shares = palette_counts(image)
                    color_rows.append((item, shares, dominant_colors(shares, {name: float(value["threshold"]) for name, value in self.color_thresholds.items()})))
            except Exception as exc:
                if isinstance(exc, FileNotFoundError) or getattr(exc, "winerror", None) in {21, 53, 67, 1167}:
                    offline.append(item)
                else:
                    errors.append((item, f"{type(exc).__name__}: {exc}"))
        embed_positions = [index for index, item in enumerate(decoded_items) if not item.get("colors_only") and not item.get("quality_only")]
        embed_images = [decoded[index] for index in embed_positions]
        embed_items = [decoded_items[index] for index in embed_positions]
        if embed_images:
            try:
                vectors = np.asarray(self._embedder.encode_images(embed_images), dtype=np.float32)
                if vectors.ndim == 1:
                    vectors = vectors.reshape(1, -1)
                if vectors.shape != (len(embed_images), DIMENSION):
                    raise ValueError(f"Model vrátil embeddingy s rozmerom {vectors.shape}")
                norms = np.linalg.norm(vectors, axis=1, keepdims=True)
                vectors = vectors / np.maximum(norms, 1e-12)
                success = list(zip(embed_items, vectors))
            except Exception as exc:
                errors.extend((item, f"{type(exc).__name__}: {exc}") for item in embed_items)
        new_indexed_count = self.indexed_count
        with self.db_lock:
            with self._condition:
                current = generation == self._generation
            if current:
                for item, vector in success:
                    self.db.execute("INSERT OR REPLACE INTO embeddings(asset_id,model,size,mtime_ns,dim,vector) VALUES(?,?,?,?,?,?)",
                                    (item["asset_id"], self.model_name, item["size"], item["mtime_ns"], DIMENSION, np.asarray(vector, dtype=np.float16).tobytes()))
                    self.db.execute("DELETE FROM failures WHERE asset_id=?", (item["asset_id"],))
                for item, shares, auto in color_rows:
                    self.db.execute("INSERT OR REPLACE INTO colors(asset_id,size,mtime_ns,shares,auto,version) VALUES(?,?,?,?,?,?)",
                                    (item["asset_id"], item["size"], item["mtime_ns"], json.dumps(shares, ensure_ascii=False), json.dumps(auto, ensure_ascii=False), COLOR_VERSION))
                for item, values in quality_rows:
                    self.db.execute("INSERT OR REPLACE INTO quality(asset_id,size,mtime_ns,sharp,hi,lo,mean,version) VALUES(?,?,?,?,?,?,?,?)",
                                    (item["asset_id"], item["size"], item["mtime_ns"], *values, QUALITY_VERSION))
                for item, error in errors:
                    self.db.execute("INSERT OR REPLACE INTO failures(asset_id,size,mtime_ns,error,at) VALUES(?,?,?,?,?)",
                                    (item["asset_id"], item["size"], item["mtime_ns"], error[:2000], time.time()))
                self.db.commit()
                new_indexed_count = int(self.db.execute("SELECT count(*) FROM embeddings WHERE model=?", (self.model_name,)).fetchone()[0])
                if success and self._vectors is not None:
                    self._vectors.upsert([item["asset_id"] for item, _ in success], np.stack([vec for _, vec in success]))
        for image in decoded:
            close = getattr(image, "close", None)
            if close:
                close()
        final = False
        with self._condition:
            if generation == self._generation:
                self._done += len(success) + sum(item.get("colors_only", False) for item, _, _ in color_rows) + sum(item.get("quality_only", False) for item, _ in quality_rows)
                self._failed += len(errors)
                self._offline += len(offline)
                self.indexed_count = new_indexed_count
                self._newly_indexed_ids.update(item["asset_id"] for item, _ in success)
                if errors:
                    self._last_error = errors[-1][1]
                final = not self._queue and not self._quality_queue
                if final:
                    self._started_at = None
            self._active = False
            now = time.monotonic()
            should_emit = final or now - self._last_progress >= 1.0
            if should_emit:
                self._last_progress = now
        if should_emit:
            self._emit({"event": "progress", **self.status()})
        self._emit_indexed_assets()

    def _learn_colors(self, request: dict[str, Any]) -> dict[str, Any]:
        corrections = request.get("corrections")
        if not isinstance(corrections, list):
            raise RequestError("corrections musí byť zoznam")
        samples: dict[str, list[tuple[float, bool]]] = {color: [] for color in COLOR_PALETTE}
        with self.db_lock:
            for correction in corrections:
                if not isinstance(correction, dict) or not isinstance(correction.get("asset_id"), int) or isinstance(correction.get("asset_id"), bool):
                    raise RequestError("Každá oprava musí obsahovať asset_id")
                color, state = correction.get("color"), correction.get("state")
                if color not in COLOR_PALETTE or not isinstance(state, bool):
                    raise RequestError("Oprava farby má neplatnú farbu alebo stav")
                row = self.db.execute("SELECT shares FROM colors WHERE asset_id=?", (correction["asset_id"],)).fetchone()
                if row is None:
                    continue
                shares = json.loads(row[0])
                samples[color].append((float(shares.get(color, 0.0)), state))
            self.color_thresholds = learn_thresholds(samples)
            self.db.execute("INSERT OR REPLACE INTO meta(key,value) VALUES('color_thresholds',?)", (json.dumps(self.color_thresholds, ensure_ascii=False),))
            rows = self.db.execute("SELECT asset_id,shares FROM colors").fetchall()
            for asset_id, shares_json in rows:
                shares = json.loads(shares_json)
                auto = dominant_colors(shares, {name: float(value["threshold"]) for name, value in self.color_thresholds.items()})
                self.db.execute("UPDATE colors SET auto=? WHERE asset_id=?", (json.dumps(auto, ensure_ascii=False), asset_id))
            self.db.commit()
        return {"thresholds": self.color_thresholds, "examples": sum(len(values) for values in samples.values())}

    def _ensure_vector_index(self) -> None:
        with self._vector_build_lock:
            while True:
                with self.db_lock:
                    if self._vectors is not None:
                        return
                    revision = self.db.total_changes
                    rows = self.db.execute("SELECT asset_id,vector FROM embeddings WHERE model=? AND dim=? ORDER BY asset_id",
                                           (self.model_name, DIMENSION)).fetchall()
                values = np.empty((len(rows), DIMENSION), dtype=np.float16)
                for i, (_, blob) in enumerate(rows):
                    values[i] = np.frombuffer(blob, dtype=np.float16)
                index = VectorIndex([int(aid) for aid, _ in rows], values)
                with self.db_lock:
                    if revision == self.db.total_changes:
                        self._vectors = index
                        return

    @staticmethod
    def _load_vector_cuda():
        try:
            import torch
            return torch if torch.cuda.is_available() else False
        except (ImportError, OSError, RuntimeError):
            return False

    def _warm_folder_cuda(self) -> None:
        with self._vector_cuda_lock:
            if self._cuda_torch is None:
                self._cuda_torch = self._load_vector_cuda()

    def _folder_vector_snapshot(self):
        # A small selection can be ranked immediately while Torch and CUDA initialize.
        with self._folder_catalog_lock:
            if self._cuda_torch is None and not self._folder_cuda_pending:
                self._folder_cuda_pending = True
                self._vector_pool.submit(self._warm_folder_cuda)
        return self._index_snapshot(use_cuda=self._cuda_torch is not None)

    def _index_snapshot(self, *, use_cuda: bool = False) -> tuple[list[int], Any, dict[int, int]]:
        if self._vectors is None:
            self._ensure_vector_index()
        with self.db_lock:
            index = self._vectors
            if not use_cuda or not index.count or self._cuda_torch is False:
                return index.snapshot()
            cached = index.cuda_snapshot()
            if cached is not None:
                return cached
        self._warm_folder_cuda()
        torch = self._cuda_torch
        with self._vector_upload_lock:
            while True:
                with self.db_lock:
                    if not torch or not index.count:
                        return index.snapshot()
                    cached = index.cuda_snapshot()
                    if cached is not None:
                        return cached
                    ids, vectors, _ = index.snapshot()
                    shape = index.matrix.shape
                device_matrix = torch.empty(shape, device="cuda", dtype=torch.float16)
                device_matrix[:len(ids)].copy_(torch.from_numpy(vectors))
                with self.db_lock:
                    # Every upsert or removal replaces ids. Publish only a matching GPU copy.
                    if index.ids is ids:
                        index._torch, index._device_matrix = torch, device_matrix
                        return index.cuda_snapshot()
                del device_matrix

    def _indexed_asset_ids(self, request):
        ids = request.get("asset_ids")
        if not isinstance(ids, list) or any(isinstance(aid, bool) or not isinstance(aid, int) for aid in ids):
            raise RequestError("asset_ids musí byť zoznam ID")
        indexed = []
        with self.db_lock:
            for start in range(0, len(ids), 500):
                chunk = ids[start:start + 500]
                marks = ",".join("?" for _ in chunk)
                indexed.extend(aid for (aid,) in self.db.execute(
                    f"SELECT asset_id FROM embeddings WHERE model=? AND dim=? AND asset_id IN ({marks})",
                    (self.model_name, DIMENSION, *chunk),
                ))
        return {"asset_ids": indexed}
