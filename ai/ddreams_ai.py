"""Persistent local SigLIP2 JSON-lines service for DDreams."""

from __future__ import annotations

import concurrent.futures
import ctypes
import io
import json
import os
import sqlite3
import sys
import threading
import time
import traceback
from pathlib import Path
from typing import Any, Callable

import numpy as np

MODEL_NAME = "ViT-L-16-SigLIP2-256"
DIMENSION = 1024
SCHEMA_VERSION = "1"
BATCH_SIZE = 32
DECODE_THREADS = 6


class RequestError(ValueError):
    pass


def _stderr(message: str) -> None:
    print(message, file=sys.stderr, flush=True)


def _set_background_priority() -> None:
    if os.name != "nt":
        return
    try:
        kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        kernel.SetPriorityClass(kernel.GetCurrentProcess(), 0x00004000)  # BELOW_NORMAL_PRIORITY_CLASS
        kernel.SetThreadPriority(kernel.GetCurrentThread(), 0x00010000)  # THREAD_MODE_BACKGROUND_BEGIN
    except Exception:
        pass


def decode_photo(item: dict[str, Any]):
    """Decode a photo to an oriented RGB PIL image suitable for model input."""
    from PIL import Image, ImageOps

    path = item["path"]
    ext = Path(path).suffix.lower().lstrip(".")
    if ext in {"arw", "dng", "cr2", "nef"}:
        import rawpy

        with rawpy.imread(path) as raw:
            thumb = raw.extract_thumb()
        if thumb.format == rawpy.ThumbFormat.JPEG:
            image = Image.open(io.BytesIO(thumb.data))
            image.load()
            image = image.convert("RGB")
        elif thumb.format == rawpy.ThumbFormat.BITMAP:
            image = Image.fromarray(np.asarray(thumb.data).reshape(thumb.height, thumb.width, 3), "RGB")
        else:
            raise ValueError(f"RAW preview format nepodporovaný: {thumb.format}")
        image = ImageOps.exif_transpose(image).convert("RGB")
        image.thumbnail((512, 512), Image.Resampling.BICUBIC)
        return image

    image = Image.open(path)
    if ext in {"jpg", "jpeg", "png"}:
        image.draft("RGB", (512, 512))
    if ext in {"tif", "tiff"}:
        image.thumbnail((512, 512), Image.Resampling.BICUBIC)
    image = ImageOps.exif_transpose(image)
    image.load()
    image = image.convert("RGB")
    image.thumbnail((512, 512), Image.Resampling.BICUBIC)
    return image


class SiglipEmbedder:
    """Real model adapter. Its in-memory search matrix lives on the CUDA device."""

    dimension = DIMENSION
    device = "cuda"

    def __init__(self, model_dir: str | os.PathLike[str]):
        os.environ["HF_HUB_OFFLINE"] = "1"
        import torch
        import open_clip
        from safetensors.torch import load_file
        from open_clip.tokenizer import HFTokenizer

        self.torch = torch
        self._lock = threading.RLock()
        self.model, _, self.preprocess = open_clip.create_model_and_transforms(
            MODEL_NAME, device="meta", image_mean=(0.5, 0.5, 0.5), image_std=(0.5, 0.5, 0.5),
            image_interpolation="bicubic", image_resize_mode="squash",
        )
        checkpoint = Path(model_dir) / "open_clip_model.fp16.safetensors"
        self.model.load_state_dict(load_file(str(checkpoint), device="cuda"), strict=False, assign=True)
        self.model.eval()
        config = open_clip.get_model_config(MODEL_NAME)
        text_cfg = config["text_cfg"]
        kwargs = text_cfg.get("tokenizer_kwargs", {})
        self.tokenizer = HFTokenizer(
            str(model_dir), context_length=int(text_cfg["context_length"]),
            clean=kwargs.get("clean", "canonicalize"),
        )
        self._ids = np.empty((0,), dtype=np.int64)
        self._matrix = torch.empty((0, DIMENSION), device="cuda", dtype=torch.float16)

    def encode_images(self, images: list[Any]) -> np.ndarray:
        torch = self.torch
        batch = torch.stack([self.preprocess(image) for image in images]).to("cuda", non_blocking=True)
        with torch.inference_mode(), torch.autocast("cuda"):
            features = self.model.encode_image(batch)
            features = features / features.norm(dim=-1, keepdim=True).clamp_min(1e-12)
        return features.float().cpu().numpy()

    def _text_vector(self, query: str):
        tokens = self.tokenizer([query]).to("cuda")
        with self.torch.inference_mode(), self.torch.autocast("cuda"):
            vector = self.model.encode_text(tokens)
            vector = vector / vector.norm(dim=-1, keepdim=True).clamp_min(1e-12)
        return vector.to(dtype=self.torch.float16)

    def replace_index(self, ids: list[int], vectors: np.ndarray) -> None:
        with self._lock:
            self._ids = np.asarray(ids, dtype=np.int64)
            self._matrix = self.torch.as_tensor(vectors, device="cuda", dtype=self.torch.float16).contiguous()

    def upsert_index(self, ids: list[int], vectors: np.ndarray) -> None:
        with self._lock:
            incoming = np.asarray(ids, dtype=np.int64)
            keep = ~np.isin(self._ids, incoming)
            matrix = self._matrix[ self.torch.as_tensor(keep, device="cuda") ]
            values = self.torch.as_tensor(vectors, device="cuda", dtype=self.torch.float16)
            self._matrix = self.torch.cat((matrix, values), dim=0).contiguous()
            self._ids = np.concatenate((self._ids[keep], incoming))

    def remove_index(self, ids: list[int]) -> None:
        if not ids:
            return
        with self._lock:
            keep = ~np.isin(self._ids, np.asarray(ids, dtype=np.int64))
            self._matrix = self._matrix[self.torch.as_tensor(keep, device="cuda")].contiguous()
            self._ids = self._ids[keep]

    def search_text(self, query: str, limit: int) -> list[dict[str, Any]]:
        with self._lock:
            if not len(self._ids):
                return []
            scores = self._matrix @ self._text_vector(query).reshape(-1)
            count = min(limit, len(self._ids))
            values, positions = self.torch.topk(scores, count)
            return [{"asset_id": int(self._ids[int(i)]), "score": float(s)} for s, i in zip(values.cpu().tolist(), positions.cpu().tolist())]

    def similar(self, asset_id: int, limit: int) -> list[dict[str, Any]]:
        with self._lock:
            matches = np.flatnonzero(self._ids == asset_id)
            if not len(matches):
                raise RequestError("Fotka ešte nie je zaindexovaná")
            scores = self._matrix @ self._matrix[int(matches[0])]
            scores[int(matches[0])] = -2.0
            count = min(limit, max(0, len(self._ids) - 1))
            values, positions = self.torch.topk(scores, count)
            return [{"asset_id": int(self._ids[int(i)]), "score": float(s)} for s, i in zip(values.cpu().tolist(), positions.cpu().tolist())]


class AIService:
    def __init__(self, db_path: str | os.PathLike[str], model_dir: str | os.PathLike[str], *,
                 embedder_factory: Callable[[str | os.PathLike[str]], Any] | None = None,
                 decoder: Callable[[dict[str, Any]], Any] = decode_photo,
                 output: Callable[[dict[str, Any]], None] | None = None):
        self.db_path = str(db_path)
        self.model_dir = str(model_dir)
        self.model_name = MODEL_NAME
        self.embedder_factory = embedder_factory or SiglipEmbedder
        self.decoder = decoder
        self.output = output or (lambda obj: None)
        self.db = sqlite3.connect(self.db_path, check_same_thread=False)
        self.db.execute("PRAGMA journal_mode=WAL")
        self.db.execute("PRAGMA synchronous=NORMAL")
        self.db.executescript("""
            CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS embeddings(asset_id INTEGER PRIMARY KEY, model TEXT NOT NULL, size INTEGER NOT NULL, mtime_ns INTEGER NOT NULL, dim INTEGER NOT NULL, vector BLOB NOT NULL);
            CREATE TABLE IF NOT EXISTS failures(asset_id INTEGER PRIMARY KEY, size INTEGER NOT NULL, mtime_ns INTEGER NOT NULL, error TEXT NOT NULL, at REAL NOT NULL);
        """)
        self.db_lock = threading.RLock()
        meta = dict(self.db.execute("SELECT key,value FROM meta").fetchall())
        if meta.get("model") != self.model_name or meta.get("schema_version") != SCHEMA_VERSION:
            self.db.execute("DELETE FROM embeddings")
            self.db.execute("DELETE FROM failures")
            self.db.execute("INSERT OR REPLACE INTO meta(key,value) VALUES('model',?)", (self.model_name,))
            self.db.execute("INSERT OR REPLACE INTO meta(key,value) VALUES('schema_version',?)", (SCHEMA_VERSION,))
        self.db.commit()
        self.indexed_count = int(self.db.execute("SELECT count(*) FROM embeddings WHERE model=?", (self.model_name,)).fetchone()[0])
        self._condition = threading.Condition()
        self._model_ready = threading.Event()
        self._model_error: str | None = None
        self._embedder: Any = None
        self._stop = False
        self._paused = False
        self._queue: list[dict[str, Any]] = []
        self._generation = 0
        self._active = False
        self._done = self._failed = self._offline = 0
        self._total = 0
        self._started_at: float | None = None
        self._last_error: str | None = None
        self._last_progress = 0.0
        self._model_load_s: float | None = None
        self._device = "cuda" if embedder_factory is None else "cpu"
        self._query_pool = concurrent.futures.ThreadPoolExecutor(max_workers=2, thread_name_prefix="ai-query")
        self._loader: threading.Thread | None = None
        self._worker: threading.Thread | None = None

    def _emit(self, obj: dict[str, Any]) -> None:
        try:
            self.output(obj)
        except Exception:
            _stderr("Nepodarilo sa odoslať protokolovú správu")

    def start(self) -> None:
        self._emit({"event": "status", **self.status()})
        self._loader = threading.Thread(target=self._load_model, name="ai-model-loader", daemon=True)
        self._worker = threading.Thread(target=self._index_loop, name="ai-indexer", daemon=True)
        self._loader.start()
        self._worker.start()

    def _load_model(self) -> None:
        started = time.perf_counter()
        try:
            _stderr("Načítavam lokálny SigLIP2 model…")
            embedder = self.embedder_factory(self.model_dir)
            self._device = str(getattr(embedder, "device", self._device))
            with self.db_lock:
                rows = self.db.execute("SELECT asset_id,vector FROM embeddings WHERE model=? AND dim=? ORDER BY asset_id", (self.model_name, DIMENSION)).fetchall()
            ids = [int(row[0]) for row in rows]
            vectors = np.stack([np.frombuffer(row[1], dtype=np.float16).astype(np.float32) for row in rows]) if rows else np.empty((0, DIMENSION), dtype=np.float32)
            embedder.replace_index(ids, vectors)
            self._embedder = embedder
            self.indexed_count = len(ids)
            self._model_ready.set()
            self._model_load_s = round(time.perf_counter() - started, 3)
            ready_status = self.status()
            ready_status["state"] = "ready"
            ready_status["model_load_s"] = self._model_load_s
            self._emit({"event": "status", **ready_status})
            _stderr(f"SigLIP2 pripravený za {self._model_load_s:.3f} s")
        except Exception as exc:
            self._model_error = f"Model sa nepodarilo načítať: {exc}"
            self._last_error = self._model_error
            self._model_ready.set()
            _stderr(traceback.format_exc())
            self._emit({"event": "status", **self.status()})
        finally:
            with self._condition:
                self._condition.notify_all()

    def status(self) -> dict[str, Any]:
        with self._condition:
            if self._model_error:
                state = "error"
            elif self._paused:
                state = "paused"
            elif not self._model_ready.is_set():
                state = "loading"
            elif self._active or self._queue:
                state = "indexing"
            else:
                state = "ready"
            queued = len(self._queue)
            elapsed = max(0.001, time.monotonic() - self._started_at) if self._started_at else 0.0
            processed = self._done + self._failed + self._offline
            rate = processed / elapsed if elapsed else 0.0
            eta = (queued + (BATCH_SIZE if self._active else 0)) / rate if rate else None
            return {"state": state, "indexed": self.indexed_count, "queued": queued, "done": self._done,
                    "failed": self._failed, "offline": self._offline, "rate_per_s": round(rate, 3),
                    "eta_s": round(eta, 1) if eta is not None else None, "last_error": self._last_error}

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
        if not isinstance(prune, bool):
            raise RequestError("prune musí byť true alebo false")
        ids = {item["asset_id"] for item in items}
        with self._condition:
            self._generation += 1
            generation = self._generation
            self._queue = []
            self._condition.notify_all()
        with self.db_lock:
            removed = 0
            if prune:
                current = {int(row[0]) for row in self.db.execute("SELECT asset_id FROM embeddings")} | {int(row[0]) for row in self.db.execute("SELECT asset_id FROM failures")}
                remove = current - ids
                removed = len(remove)
                if remove:
                    marks = ",".join("?" for _ in remove)
                    self.db.execute(f"DELETE FROM embeddings WHERE asset_id IN ({marks})", tuple(remove))
                    self.db.execute(f"DELETE FROM failures WHERE asset_id IN ({marks})", tuple(remove))
                self.indexed_count = int(self.db.execute("SELECT count(*) FROM embeddings WHERE model=?", (self.model_name,)).fetchone()[0])
            old = {int(a): (str(m), int(s), int(t)) for a, m, s, t in self.db.execute("SELECT asset_id,model,size,mtime_ns FROM embeddings")}
            failed_old = {int(a): (int(s), int(t)) for a, s, t in self.db.execute("SELECT asset_id,size,mtime_ns FROM failures")}
            queued, upto, failed_before = [], 0, 0
            for item in items:
                aid, size, mtime = item["asset_id"], item["size"], item["mtime_ns"]
                if old.get(aid) == (self.model_name, size, mtime):
                    upto += 1
                elif failed_old.get(aid) == (size, mtime):
                    failed_before += 1
                else:
                    queued.append(item)
                    if aid in failed_old:
                        self.db.execute("DELETE FROM failures WHERE asset_id=?", (aid,))
            self.db.commit()
        if removed and self._embedder is not None:
            self._embedder.remove_index(list(current - ids))
        with self._condition:
            self._generation = generation
            self._queue = queued
            self._done = self._failed = self._offline = 0
            self._total = len(queued)
            self._started_at = time.monotonic() if queued else None
            self._last_error = None
            self._condition.notify_all()
        return {"queued": len(queued), "up_to_date": upto, "failed_before": failed_before, "pruned": removed}

    def _index_loop(self) -> None:
        _set_background_priority()
        pool = concurrent.futures.ThreadPoolExecutor(max_workers=DECODE_THREADS, thread_name_prefix="ai-decode", initializer=_set_background_priority)
        try:
            while True:
                with self._condition:
                    while not self._stop and (self._paused or not self._model_ready.is_set() or self._model_error or not self._queue):
                        self._condition.wait(timeout=0.5)
                    if self._stop:
                        return
                    generation = self._generation
                    batch, self._queue = self._queue[:BATCH_SIZE], self._queue[BATCH_SIZE:]
                    self._active = True
                self._process_batch(batch, generation, pool)
        finally:
            pool.shutdown(wait=True)

    def _process_batch(self, batch: list[dict[str, Any]], generation: int, pool: concurrent.futures.ThreadPoolExecutor) -> None:
        success, errors, offline = [], [], []
        futures = {}
        for item in batch:
            if not os.path.isfile(item["path"]):
                offline.append(item)
            else:
                futures[pool.submit(self.decoder, item)] = item
        decoded, decoded_items = [], []
        for future, item in futures.items():
            try:
                decoded.append(future.result())
                decoded_items.append(item)
            except Exception as exc:
                if isinstance(exc, FileNotFoundError) or getattr(exc, "winerror", None) in {21, 53, 67, 1167}:
                    offline.append(item)
                else:
                    errors.append((item, f"{type(exc).__name__}: {exc}"))
        vectors = np.empty((0, DIMENSION), dtype=np.float32)
        if decoded:
            try:
                vectors = np.asarray(self._embedder.encode_images(decoded), dtype=np.float32)
                if vectors.ndim == 1:
                    vectors = vectors.reshape(1, -1)
                if vectors.shape != (len(decoded), DIMENSION):
                    raise ValueError(f"Model vrátil embeddingy s rozmerom {vectors.shape}")
                norms = np.linalg.norm(vectors, axis=1, keepdims=True)
                vectors = vectors / np.maximum(norms, 1e-12)
                success = list(zip(decoded_items, vectors))
            except Exception as exc:
                errors.extend((item, f"{type(exc).__name__}: {exc}") for item in decoded_items)
        new_indexed_count = self.indexed_count
        with self.db_lock:
            with self._condition:
                current = generation == self._generation
            if current:
                for item, vector in success:
                    self.db.execute("INSERT OR REPLACE INTO embeddings(asset_id,model,size,mtime_ns,dim,vector) VALUES(?,?,?,?,?,?)",
                                    (item["asset_id"], self.model_name, item["size"], item["mtime_ns"], DIMENSION, np.asarray(vector, dtype=np.float16).tobytes()))
                    self.db.execute("DELETE FROM failures WHERE asset_id=?", (item["asset_id"],))
                for item, error in errors:
                    self.db.execute("INSERT OR REPLACE INTO failures(asset_id,size,mtime_ns,error,at) VALUES(?,?,?,?,?)",
                                    (item["asset_id"], item["size"], item["mtime_ns"], error[:2000], time.time()))
                self.db.commit()
                new_indexed_count = int(self.db.execute("SELECT count(*) FROM embeddings WHERE model=?", (self.model_name,)).fetchone()[0])
                if success:
                    self._embedder.upsert_index([item["asset_id"] for item, _ in success], np.stack([vec for _, vec in success]))
        for image in decoded:
            close = getattr(image, "close", None)
            if close:
                close()
        final = False
        with self._condition:
            if generation == self._generation:
                self._done += len(success)
                self._failed += len(errors)
                self._offline += len(offline)
                self.indexed_count = new_indexed_count
                if errors:
                    self._last_error = errors[-1][1]
                final = not self._queue
                if final:
                    self._started_at = None
            self._active = False
            now = time.monotonic()
            should_emit = final or now - self._last_progress >= 1.0
            if should_emit:
                self._last_progress = now
        if should_emit:
            self._emit({"event": "progress", **self.status()})

    def _wait_model(self) -> Any:
        self._model_ready.wait()
        if self._model_error:
            raise RequestError(self._model_error)
        if self._stop:
            raise RequestError("Server sa vypína")
        return self._embedder

    def execute(self, request: dict[str, Any]) -> dict[str, Any]:
        cmd = request.get("cmd")
        if cmd == "hello":
            return {"protocol": 1, "model": self.model_name, "dim": DIMENSION, "device": self._device,
                    "state": self.status()["state"], "indexed": self.indexed_count}
        if cmd == "status":
            return self.status()
        if cmd == "index":
            return self._index_command(request)
        if cmd == "pause":
            with self._condition:
                self._paused = True
                self._condition.notify_all()
            return self.status()
        if cmd == "resume":
            with self._condition:
                self._paused = False
                self._condition.notify_all()
            return self.status()
        if cmd == "search_text":
            query = request.get("query")
            if not isinstance(query, str) or not query.strip():
                raise RequestError("query musí byť neprázdny text")
            limit = self._limit(request.get("limit", 200))
            return self._wait_model().search_text(query.strip(), limit)
        if cmd == "similar":
            aid = request.get("asset_id")
            if isinstance(aid, bool) or not isinstance(aid, int) or aid <= 0:
                raise RequestError("asset_id musí byť kladné celé číslo")
            limit = self._limit(request.get("limit", 200))
            return self._wait_model().similar(aid, limit)
        if cmd == "shutdown":
            return {"shutting_down": True}
        raise RequestError(f"Neznámy príkaz: {cmd}")

    @staticmethod
    def _limit(value: Any) -> int:
        if isinstance(value, bool) or not isinstance(value, int) or not 1 <= value <= 1000:
            raise RequestError("limit musí byť celé číslo od 1 do 1000")
        return value

    def _respond(self, writer: Any, lock: threading.Lock, response: dict[str, Any]) -> None:
        with lock:
            writer.write(json.dumps(response, ensure_ascii=False, separators=(",", ":")) + "\n")
            writer.flush()

    def serve(self, reader: Any = None, writer: Any = None) -> None:
        reader = reader or sys.stdin
        writer = writer or sys.stdout
        write_lock = threading.Lock()
        self.output = lambda obj: self._respond(writer, write_lock, obj)
        self.start()

        def query_job(request: dict[str, Any], request_id: int) -> None:
            try:
                result = self.execute(request)
                response = {"id": request_id, "ok": True, "result": result}
            except Exception as exc:
                response = {"id": request_id, "ok": False, "error": str(exc)}
            self._respond(writer, write_lock, response)

        try:
            for line in reader:
                if not line.strip():
                    continue
                request_id = None
                try:
                    request = json.loads(line)
                    if not isinstance(request, dict):
                        raise RequestError("Požiadavka musí byť JSON objekt")
                    request_id = request.get("id")
                    if isinstance(request_id, bool) or not isinstance(request_id, int):
                        raise RequestError("id musí byť celé číslo")
                    if request.get("cmd") in {"search_text", "similar"}:
                        self._query_pool.submit(query_job, request, request_id)
                        continue
                    result = self.execute(request)
                    response = {"id": request_id, "ok": True, "result": result}
                    self._respond(writer, write_lock, response)
                    if request.get("cmd") == "shutdown":
                        break
                except Exception as exc:
                    self._respond(writer, write_lock, {"id": request_id, "ok": False, "error": str(exc)})
        finally:
            self.stop()

    def stop(self) -> None:
        self._query_pool.shutdown(wait=True)
        with self._condition:
            self._stop = True
            self._condition.notify_all()
        if self._loader and self._loader is not threading.current_thread():
            self._loader.join()
        if self._worker and self._worker is not threading.current_thread():
            self._worker.join()
        with self.db_lock:
            self.db.commit()
            self.db.close()


def _main(argv: list[str] | None = None) -> int:
    import argparse

    parser = argparse.ArgumentParser(description="DDreams local SigLIP2 JSON-lines server")
    parser.add_argument("--model", required=True, help="Lokálny priečinok modelu")
    parser.add_argument("--db", required=True, help="Cesta k databáze ai.sqlite")
    args = parser.parse_args(argv)
    os.environ["HF_HUB_OFFLINE"] = "1"
    try:
        sys.stdin.reconfigure(encoding="utf-8", errors="replace")
        sys.stdout.reconfigure(encoding="utf-8", errors="replace", newline="\n")
        sys.stderr.reconfigure(encoding="utf-8", errors="backslashreplace")
    except (AttributeError, OSError):
        pass
    try:
        AIService(args.db, args.model).serve()
        return 0
    except Exception as exc:
        _stderr(f"Fatálna chyba servera: {exc}")
        return 1


if __name__ == "__main__":
    raise SystemExit(_main())
