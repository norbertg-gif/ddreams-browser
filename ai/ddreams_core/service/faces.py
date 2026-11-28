from __future__ import annotations

import os
import sqlite3
import sys
import time
from pathlib import Path
from typing import Any
import numpy as np

from ..constants import (
    FACE_VERSION,
    FACE_IMAGE_MAX,
    FACE_MIN_SIDE,
    FACE_MATCH_SIM,
    FACE_STRONG_SIM,
    FACE_TOP_K,
    FACE_CLUSTER_MAX,
    FACE_CLUSTER_SIM,
    FACE_CLUSTER_MIN_SIZE,
    FACE_CLUSTER_GUESS_SIM,
)
from ..util import RequestError, _stderr, _set_background_priority
from ..decode import decode_photo
from ..face_models import face_detect, face_align, _box_iou


class FacesMixin:
    def _ensure_face_models(self) -> bool:
        if self._faces_available:
            return True
        if self._faces_error or self.faces_db is None:
            return False
        with self._face_model_lock:
            if self._faces_available:
                return True
            try:
                embedder = self._wait_model()
                torch = getattr(embedder, "torch", None)
                if torch is None:
                    raise RuntimeError("Torch ešte nie je dostupné pre onnxruntime-gpu")
                base = Path(self.model_dir).parent
                face_dir, py_dir = base / "faces", base / "pylibs"
                det_path, emb_path = face_dir / "det_10g.onnx", face_dir / "w600k_r50.onnx"
                if not face_dir.is_dir() or not py_dir.is_dir() or not det_path.is_file() or not emb_path.is_file():
                    raise FileNotFoundError("Lokálne face modely alebo pylibs chýbajú")
                if str(py_dir) not in sys.path:
                    sys.path.insert(0, str(py_dir))
                import onnxruntime as ort
                ort.set_default_logger_severity(4)
                providers = ["CUDAExecutionProvider", "CPUExecutionProvider"]
                self._face_detector = ort.InferenceSession(str(det_path), providers=providers)
                self._face_encoder = ort.InferenceSession(str(emb_path), providers=providers)
                self._face_det_input = self._face_detector.get_inputs()[0].name
                self._face_encoder_input = self._face_encoder.get_inputs()[0].name
                self._faces_available = True
                with self.faces_lock:
                    self._refresh_face_references_locked()
                _stderr("Lokálne face modely pripravené")
            except Exception as exc:
                self._faces_error = str(exc)
                self._faces_available = False
                _stderr(f"Face modely nedostupné: {exc}")
        return self._faces_available

    def _face_vectors(self, crops: list[Any]) -> np.ndarray:
        if hasattr(self._face_encoder, "embed"):
            values = self._face_encoder.embed(crops)
        else:
            blob = np.stack([(np.asarray(c, dtype=np.float32) - 127.5) / 127.5 for c in crops]).transpose(0, 3, 1, 2)
            values = np.concatenate([self._face_encoder.run(None, {self._face_encoder_input: blob[i:i + 1]})[0] for i in range(len(blob))])
        values = np.asarray(values, dtype=np.float32).reshape(len(crops), 512)
        return values / np.maximum(np.linalg.norm(values, axis=1, keepdims=True), 1e-12)

    def _face_detections(self, image: Any):
        if hasattr(self._face_detector, "detect"):
            return self._face_detector.detect(image)
        return face_detect(image, self._face_detector, self._face_det_input)

    def _face_scan_loop(self) -> None:
        _set_background_priority()
        while True:
            with self._condition:
                while not self._stop and (self._paused or not self._model_ready.is_set() or self._model_error or not self._face_scan_queue):
                    self._condition.wait(timeout=0.5)
                if self._stop:
                    return
                item = self._face_scan_queue.popleft()
                self._face_active_ids.add(item["asset_id"])
            self._model_ready.wait()
            with self._condition:
                while not self._stop and self._paused:
                    self._condition.wait(timeout=0.5)
                if self._stop: return
            if not self._ensure_face_models():
                with self._condition:
                    self._face_active_ids.discard(item["asset_id"])
                    self._face_scan_queue.clear()
                self._emit_indexed_assets()
                self._emit({"event": "status", **self.status()})
                continue
            try:
                if not os.path.isfile(item["path"]):
                    continue
                if self.decoder is decode_photo:
                    image = decode_photo(item, FACE_IMAGE_MAX)
                else:
                    image = self.decoder(item)
                try:
                    self._scan_face_asset(item, image)
                finally:
                    close = getattr(image, "close", None)
                    if close: close()
                now = time.monotonic()
                with self._condition:
                    emit_progress = not self._face_scan_queue or now - self._last_face_progress >= 1.0
                    if emit_progress: self._last_face_progress = now
                if emit_progress: self._emit({"event": "status", **self.status()})
            except Exception as exc:
                _stderr(f"Sken tvárí pre {item['asset_id']} zlyhal: {exc}")
                # Do not mark a missing or changed file: it must be retried when available.
                try:
                    if isinstance(exc, FileNotFoundError) or getattr(exc, "winerror", None) in {21, 53, 67, 1167}:
                        continue
                    stat = os.stat(item["path"])
                    if stat.st_size == item["size"] and stat.st_mtime_ns == item["mtime_ns"] and self.faces_db is not None:
                        with self.faces_lock:
                            self.faces_db.rollback()
                            self.faces_db.execute("""INSERT INTO face_scan(asset_id,size,mtime_ns,version,faces) VALUES(?,?,?,?,?)
                                ON CONFLICT(asset_id) DO UPDATE SET size=excluded.size,mtime_ns=excluded.mtime_ns,version=excluded.version,faces=0
                                WHERE face_scan.size IS NOT excluded.size OR face_scan.mtime_ns IS NOT excluded.mtime_ns OR face_scan.version IS NOT excluded.version""",
                                                  (item["asset_id"], item["size"], item["mtime_ns"], FACE_VERSION, 0))
                            self.faces_db.commit()
                except OSError:
                    pass
                except sqlite3.Error as error:
                    _stderr(f"Záznam neúspešného skenu sa nepodaril: {error}")
            finally:
                with self._condition:
                    self._face_active_ids.discard(item["asset_id"])
                    self._condition.notify_all()
                self._emit_indexed_assets()
                # Faces scanned before their references existed stay unknown; re-run naming now and then.
                self._face_scans_since_recognize = getattr(self, "_face_scans_since_recognize", 0) + 1
                with self._condition:
                    drained = not self._face_scan_queue
                if self._face_scans_since_recognize >= 500 or (drained and self._face_scans_since_recognize > 0):
                    self._face_scans_since_recognize = 0
                    try:
                        self._faces_recognize_all()
                    except Exception as exc:
                        _stderr(f"Prepočet rozpoznania tvárí zlyhal: {exc}")

    def _scan_face_asset(self, item: dict[str, Any], image: Any) -> int:
        from PIL import Image
        rgb = image.convert("RGB")
        boxes, scores, kpss = self._face_detections(rgb)
        with self.faces_lock:
            existing = self.faces_db.execute("SELECT id,x,y,w,h,source,person_id,state FROM faces WHERE asset_id=?", (item["asset_id"],)).fetchall()
        candidates = []
        for i, box in enumerate(boxes):
            x1, y1, x2, y2 = map(float, box)
            norm_box = (x1 / rgb.width, y1 / rgb.height, (x2 - x1) / rgb.width, (y2 - y1) / rgb.height)
            for row in existing:
                if any(value is None for value in row[1:5]):
                    continue
                iou = _box_iou(norm_box, (float(row[1]), float(row[2]), float(row[3]), float(row[4])))
                if iou >= 0.3 and (min(x2 - x1, y2 - y1) >= FACE_MIN_SIDE or row[5] == "acdsee"):
                    candidates.append((iou, i, row))
        used_det, used_row, matches = set(), set(), {}
        for iou, di, row in sorted(candidates, key=lambda v: (-v[0], -float(scores[v[1]]), int(v[2][0]))):
            if di not in used_det and int(row[0]) not in used_row:
                used_det.add(di); used_row.add(int(row[0])); matches[di] = row
        for i, box in enumerate(boxes):
            if i not in used_det and min(float(box[2] - box[0]), float(box[3] - box[1])) < FACE_MIN_SIDE:
                used_det.add(i)
        keep = [i for i in range(len(boxes)) if i not in used_det or i in matches]
        crops = [face_align(rgb, np.asarray(kpss[i])) for i in keep]
        vectors = self._face_vectors(crops) if crops else np.empty((0, 512), dtype=np.float32)
        import io as _io
        updates = []
        for pos, i in enumerate(keep):
            crop = crops[pos].resize((96, 96), Image.Resampling.BILINEAR)
            buffer = _io.BytesIO(); crop.save(buffer, format="JPEG", quality=80)
            x1, y1, x2, y2 = map(float, boxes[i]); norm = (x1 / rgb.width, y1 / rgb.height, (x2 - x1) / rgb.width, (y2 - y1) / rgb.height)
            updates.append((i, norm, float(scores[i]), vectors[pos].astype(np.float16).tobytes(), buffer.getvalue()))
        with self.faces_lock:
            for i, norm, score, vector, thumb in updates:
                row = matches.get(i)
                if row:
                    fid = int(row[0]); use_box = (float(row[1]), float(row[2]), float(row[3]), float(row[4])) if row[5] == "acdsee" else norm
                    self.faces_db.execute("UPDATE faces SET x=?,y=?,w=?,h=?,det_score=?,vector=? WHERE id=?", (*use_box, score, vector, fid))
                else:
                    cur = self.faces_db.execute("INSERT INTO faces(asset_id,x,y,w,h,det_score,source,ext_id,person_id,state,score,vector) VALUES(?,?,?,?,?,?,'ai',NULL,NULL,'unknown',NULL,?)",
                                                (item["asset_id"], *norm, score, vector))
                    fid = int(cur.lastrowid)
                self.faces_db.execute("INSERT OR REPLACE INTO face_thumbs(face_id,jpeg) VALUES(?,?)", (fid, thumb))
            matched = {int(row[0]) for row in matches.values()}
            for row in existing:
                fid, source, state = int(row[0]), row[5], row[7]
                if source == "ai" and fid not in matched and state in ("unknown", "auto"):
                    self.faces_db.execute("DELETE FROM face_thumbs WHERE face_id=?", (fid,))
                    self.faces_db.execute("DELETE FROM face_rejected WHERE face_id=?", (fid,))
                    self.faces_db.execute("DELETE FROM faces WHERE id=?", (fid,))
            self.faces_db.execute("INSERT OR REPLACE INTO face_scan(asset_id,size,mtime_ns,version,faces) VALUES(?,?,?,?,?)",
                                  (item["asset_id"], item["size"], item["mtime_ns"], FACE_VERSION, len(updates)))
            self.faces_db.commit()
            self._refresh_face_references_locked()
            ids = [int(row[0]) for row in self.faces_db.execute("SELECT id FROM faces WHERE asset_id=? AND state IN ('unknown','auto','suggested') AND vector IS NOT NULL", (item["asset_id"],))]
        self._recognize_face_ids(ids)
        return len(updates)

    def _refresh_face_references_locked(self) -> None:
        if self.faces_db is None:
            self._face_refs_ids = np.empty(0, dtype=np.int64); self._face_refs_people = np.empty(0, dtype=np.int64)
            self._face_refs_matrix = np.empty((0, 512), dtype=np.float32); return
        rows = self.faces_db.execute("SELECT id,person_id,vector FROM faces WHERE state IN ('confirmed','acdsee') AND vector IS NOT NULL AND person_id IS NOT NULL ORDER BY id").fetchall()
        self._face_refs_ids = np.asarray([int(row[0]) for row in rows], dtype=np.int64)
        self._face_refs_people = np.asarray([int(row[1]) for row in rows], dtype=np.int64)
        self._face_refs_matrix = np.stack([np.frombuffer(row[2], dtype=np.float16).astype(np.float32) for row in rows]) if rows else np.empty((0, 512), dtype=np.float32)

    @staticmethod
    def _recognition_choice(face_id: int, vector: np.ndarray, ref_ids: np.ndarray, ref_people: np.ndarray,
                            ref_matrix: np.ndarray, rejected: set[tuple[int, int]]) -> tuple[int | None, float | None]:
        if not len(ref_ids): return None, None
        similarities = vector if vector.shape == (len(ref_ids),) else ref_matrix @ vector
        valid = (ref_ids != face_id) & (similarities >= FACE_MATCH_SIM)
        if rejected:
            valid &= np.fromiter(((face_id, int(person)) not in rejected for person in ref_people), dtype=bool, count=len(ref_people))
        indices = np.flatnonzero(valid)
        if not len(indices): return None, None
        indices = indices[np.argsort(-similarities[indices], kind="stable")[:FACE_TOP_K]]
        sums: dict[int, float] = {}; counts: dict[int, int] = {}; best: dict[int, float] = {}
        for i in indices:
            person = int(ref_people[i]); sim = float(similarities[i])
            sums[person] = sums.get(person, 0.0) + sim; counts[person] = counts.get(person, 0) + 1
            best[person] = max(best.get(person, -1.0), sim)
        winner = min(sums, key=lambda person: (-sums[person], person))
        if counts[winner] >= 2 or best[winner] >= FACE_STRONG_SIM:
            return winner, best[winner]
        return None, None

    def _recognize_face_ids(self, face_ids: list[int]) -> int:
        if not face_ids or self.faces_db is None: return 0
        pending = []
        with self.faces_lock:
            ref_ids = self._face_refs_ids.copy(); ref_people = self._face_refs_people.copy(); ref_matrix = self._face_refs_matrix.copy()
            for start in range(0, len(face_ids), 500):
                chunk = face_ids[start:start + 500]; marks = ",".join("?" for _ in chunk)
                rows = self.faces_db.execute(f"SELECT id,vector FROM faces WHERE id IN ({marks}) AND vector IS NOT NULL", chunk).fetchall()
                rejected_rows = self.faces_db.execute(f"SELECT face_id,person_id FROM face_rejected WHERE face_id IN ({marks})", chunk).fetchall()
                rejects: dict[int, set[tuple[int, int]]] = {}
                for face_id, person_id in rejected_rows:
                    rejects.setdefault(int(face_id), set()).add((int(face_id), int(person_id)))
                for fid, blob in rows:
                    pending.append((int(fid), np.frombuffer(blob, dtype=np.float16).astype(np.float32), rejects.get(int(fid), set())))
        changes = []
        for start in range(0, len(pending), 256):
            block = pending[start:start + 256]
            if not block: continue
            vectors = np.stack([row[1] for row in block])
            sims = vectors @ ref_matrix.T if len(ref_ids) else np.empty((len(block), 0), dtype=np.float32)
            for row, vector_sims in zip(block, sims):
                fid, _, rejected = row
                person, score = self._recognition_choice(fid, vector_sims, ref_ids, ref_people, ref_matrix, rejected)
                changes.append((person, "auto" if person is not None else "unknown", score, fid))
        with self.faces_lock:
            if changes:
                self.faces_db.executemany("UPDATE faces SET person_id=?,state=?,score=? WHERE id=?", changes)
                self.faces_db.commit()
        return len(changes)

    def _faces_status(self) -> dict[str, Any]:
        available = self._faces_data_ready()
        if not available or self.faces_db is None:
            return {"available": False, "scanned": 0, "pending": 0, "faces": 0, "named": 0, "unknown": 0, "persons": 0}
        with self.faces_lock:
            scanned = int(self.faces_db.execute("SELECT count(*) FROM face_scan").fetchone()[0])
            faces = int(self.faces_db.execute("SELECT count(*) FROM faces").fetchone()[0])
            named = int(self.faces_db.execute("SELECT count(*) FROM faces WHERE person_id IS NOT NULL AND state!='ignored'").fetchone()[0])
            unknown = int(self.faces_db.execute("SELECT count(*) FROM faces WHERE state IN ('unknown','suggested')").fetchone()[0])
            persons = int(self.faces_db.execute("SELECT count(*) FROM persons").fetchone()[0])
        with self._condition: pending = len(self._face_scan_queue) + len(self._face_active_ids)
        return {"available": True, "scanned": scanned, "pending": pending, "faces": faces, "named": named, "unknown": unknown, "persons": persons}

    @staticmethod
    def _face_ids_arg(request: dict[str, Any], key: str, limit: int = 500) -> list[int]:
        values = request.get(key)
        if not isinstance(values, list) or len(values) > limit or any(isinstance(v, bool) or not isinstance(v, int) or v <= 0 for v in values):
            raise RequestError(f"{key} musí byť zoznam kladných ID (max. {limit})")
        return list(dict.fromkeys(values))

    def _faces_persons(self) -> dict[str, Any]:
        with self.faces_lock:
            rows = self.faces_db.execute("""SELECT p.id,p.name,
                sum(CASE WHEN f.state IN ('confirmed','acdsee') THEN 1 ELSE 0 END),
                sum(CASE WHEN f.state='auto' THEN 1 ELSE 0 END),
                count(DISTINCT CASE WHEN f.state IN ('confirmed','acdsee','auto') THEN f.asset_id END),
                (SELECT f2.id FROM faces f2 JOIN face_thumbs t ON t.face_id=f2.id WHERE f2.person_id=p.id AND f2.state IN ('confirmed','acdsee') ORDER BY f2.id LIMIT 1)
                FROM persons p LEFT JOIN faces f ON f.person_id=p.id GROUP BY p.id,p.name""").fetchall()
        rows.sort(key=lambda r: str(r[1]).casefold())
        return {"persons": [{"id": int(r[0]), "name": r[1], "confirmed": int(r[2] or 0), "auto": int(r[3] or 0), "assets": int(r[4] or 0), "cover": int(r[5]) if r[5] is not None else None} for r in rows]}

    def _faces_person_assets(self, request: dict[str, Any]) -> dict[str, Any]:
        person = request.get("person_id"); include_auto = request.get("include_auto", True)
        if isinstance(person, bool) or not isinstance(person, int) or person <= 0 or not isinstance(include_auto, bool): raise RequestError("Neplatné person_id/include_auto")
        states = "('confirmed','acdsee','auto')" if include_auto else "('confirmed','acdsee')"
        with self.faces_lock:
            ids = [int(r[0]) for r in self.faces_db.execute(f"SELECT DISTINCT asset_id FROM faces WHERE person_id=? AND state IN {states} ORDER BY asset_id", (person,))]
        return {"asset_ids": ids}

    def _faces_review(self, request: dict[str, Any]) -> dict[str, Any]:
        person = request.get("person_id"); limit = request.get("limit", 200)
        if isinstance(person, bool) or not isinstance(person, int) or person <= 0: raise RequestError("person_id musí byť kladné ID")
        if isinstance(limit, bool) or not isinstance(limit, int) or not 1 <= limit <= 1000: raise RequestError("limit musí byť od 1 do 1000")
        with self.faces_lock:
            rows = self.faces_db.execute("SELECT id,asset_id,score FROM faces WHERE person_id=? AND state='auto' ORDER BY score ASC,id LIMIT ?", (person, limit)).fetchall()
        return {"faces": [{"face_id": int(a), "asset_id": int(b), "score": float(c) if c is not None else None} for a, b, c in rows]}

    def _faces_clusters(self, request: dict[str, Any]) -> dict[str, Any]:
        min_size = request.get("min_size", FACE_CLUSTER_MIN_SIZE)
        min_sim = request.get("min_sim", FACE_CLUSTER_SIM)
        limit = request.get("limit", 30)
        if isinstance(min_size, bool) or not isinstance(min_size, int) or min_size < 2:
            raise RequestError("min_size musí byť celé číslo aspoň 2")
        if isinstance(min_sim, bool) or not isinstance(min_sim, (int, float)) or not np.isfinite(min_sim) or not 0.3 <= min_sim <= 0.9:
            raise RequestError("min_sim musí byť od 0.3 do 0.9")
        if isinstance(limit, bool) or not isinstance(limit, int) or not 1 <= limit <= 100:
            raise RequestError("limit musí byť od 1 do 100")
        if self.faces_db is None:
            return {"available": False, "groups": [], "unknown": 0, "clustered": 0}
        with self.faces_lock:
            rows = self.faces_db.execute("SELECT id,asset_id,det_score,vector FROM faces WHERE state='unknown' AND person_id IS NULL AND vector IS NOT NULL ORDER BY id").fetchall()
            ref_ids = self._face_refs_ids.copy()
            ref_people = self._face_refs_people.copy()
            ref_matrix = self._face_refs_matrix.copy()
            ref_names = {int(r[0]): str(r[1]) for r in self.faces_db.execute("SELECT id,name FROM persons")}
        if len(rows) > FACE_CLUSTER_MAX:
            rows = sorted(rows, key=lambda r: (-(float(r[2]) if r[2] is not None else -1.0), int(r[0])))[:FACE_CLUSTER_MAX]
            rows.sort(key=lambda r: int(r[0]))
        n = len(rows)
        if n < min_size:
            return {"available": True, "groups": [], "unknown": n, "clustered": 0}
        face_ids = np.asarray([int(r[0]) for r in rows], dtype=np.int64)
        asset_by_id = {int(r[0]): int(r[1]) for r in rows}
        matrix = np.stack([np.frombuffer(r[3], dtype=np.float16).astype(np.float32) for r in rows])
        neighbours: list[list[tuple[int, float]]] = [[] for _ in range(n)]
        for start in range(0, n, 2048):
            scores = matrix[start:start + 2048] @ matrix.T
            rr, cc = np.nonzero(scores >= float(min_sim))
            for r, c in zip(rr.tolist(), cc.tolist()):
                i = start + r
                if i != c:
                    neighbours[i].append((c, float(scores[r, c])))
        order = sorted(range(n), key=lambda i: (-len(neighbours[i]), int(face_ids[i])))
        assigned: set[int] = set()
        groups: list[dict[str, Any]] = []
        for seed in order:
            if seed in assigned:
                continue
            members = [(i, sim) for i, sim in neighbours[seed] if i not in assigned]
            if not members:
                continue
            assigned.add(seed)
            assigned.update(i for i, _ in members)
            group_indices = [seed, *(i for i, _ in sorted(members, key=lambda pair: (-pair[1], int(face_ids[pair[0]]))))]
            if len(group_indices) < min_size:
                continue
            group_ids = [int(face_ids[i]) for i in group_indices]
            guess = None
            if len(ref_ids):
                similarities = ref_matrix @ matrix[seed]
                best = int(np.argmax(similarities))
                score = float(similarities[best])
                if score >= FACE_CLUSTER_GUESS_SIM:
                    person_id = int(ref_people[best])
                    guess = {"person_id": person_id, "name": ref_names.get(person_id, ""), "score": score}
            group_assets = sorted({asset_by_id[fid] for fid in group_ids})
            groups.append({"face_ids": group_ids[:300], "total": len(group_ids), "asset_ids": group_assets[:2000], "assets": len(group_assets), "guess": guess, "_seed": int(face_ids[seed])})
        groups.sort(key=lambda g: (-g["total"], g["_seed"]))
        for group in groups:
            del group["_seed"]
        groups = groups[:limit]
        return {"available": True, "groups": groups, "unknown": n, "clustered": sum(g["total"] for g in groups)}

    def _faces_for_assets(self, request: dict[str, Any]) -> dict[str, Any]:
        asset_ids = self._face_ids_arg(request, "asset_ids")
        results = []
        with self.faces_lock:
            for start in range(0, len(asset_ids), 400):
                chunk = asset_ids[start:start + 400]; marks = ",".join("?" for _ in chunk)
                rows = self.faces_db.execute(f"SELECT f.id,f.asset_id,f.x,f.y,f.w,f.h,f.person_id,p.name,f.state,f.score FROM faces f LEFT JOIN persons p ON p.id=f.person_id WHERE f.asset_id IN ({marks}) AND f.state!='ignored' ORDER BY f.asset_id,f.id", chunk).fetchall()
                results.extend({"face_id": int(r[0]), "asset_id": int(r[1]), "x": r[2], "y": r[3], "w": r[4], "h": r[5], "person_id": int(r[6]) if r[6] is not None else None, "name": r[7], "state": r[8], "score": r[9]} for r in rows)
        return {"faces": results}

    def _faces_set(self, request: dict[str, Any]) -> dict[str, Any]:
        ids = self._face_ids_arg(request, "face_ids"); action = request.get("action"); person = request.get("person_id")
        if action not in ("confirm", "reject", "assign", "clear", "ignore"): raise RequestError("Neplatná face action")
        if action == "assign" and (isinstance(person, bool) or not isinstance(person, int) or person <= 0): raise RequestError("assign vyžaduje person_id")
        recognize = []
        changed = 0
        with self.faces_lock:
            if action == "assign" and self.faces_db.execute("SELECT 1 FROM persons WHERE id=?", (person,)).fetchone() is None: raise RequestError("Osoba neexistuje")
            for start in range(0, len(ids), 400):
                chunk = ids[start:start + 400]; marks = ",".join("?" for _ in chunk)
                rows = self.faces_db.execute(f"SELECT id,person_id FROM faces WHERE id IN ({marks})", chunk).fetchall() if chunk else []
                for fid, current in rows:
                    fid = int(fid)
                    if action == "confirm":
                        if current is not None: self.faces_db.execute("UPDATE faces SET state='confirmed',score=NULL WHERE id=?", (fid,)); changed += 1
                    elif action == "assign": self.faces_db.execute("UPDATE faces SET person_id=?,state='confirmed',score=NULL WHERE id=?", (person, fid)); changed += 1
                    elif action == "reject":
                        if current is not None: self.faces_db.execute("INSERT OR IGNORE INTO face_rejected(face_id,person_id) VALUES(?,?)", (fid, current))
                        self.faces_db.execute("UPDATE faces SET person_id=NULL,state='unknown',score=NULL WHERE id=?", (fid,)); recognize.append(fid); changed += 1
                    elif action == "clear": self.faces_db.execute("UPDATE faces SET person_id=NULL,state='unknown',score=NULL WHERE id=?", (fid,)); changed += 1
                    else: self.faces_db.execute("UPDATE faces SET person_id=NULL,state='ignored',score=NULL WHERE id=?", (fid,)); changed += 1
            self.faces_db.commit(); self._refresh_face_references_locked()
        if recognize: self._recognize_face_ids(recognize)
        return {"changed": changed}

    def _faces_asset_ids_arg(self, request: dict[str, Any]) -> list[int]:
        ids=request.get("asset_ids")
        if not isinstance(ids,list) or len(ids)>2000 or any(isinstance(a,bool) or not isinstance(a,int) or a<=0 for a in ids): raise RequestError("asset_ids musí obsahovať najviac 2000 kladných ID")
        return list(dict.fromkeys(ids))

    def _faces_asset_assign(self, request: dict[str, Any]) -> dict[str, Any]:
        ids=self._faces_asset_ids_arg(request);person=request.get("person_id")
        if isinstance(person,bool) or not isinstance(person,int) or person<=0: raise RequestError("person_id musí byť kladné ID")
        changed=0
        with self.faces_lock:
            if self.faces_db.execute("SELECT 1 FROM persons WHERE id=?",(person,)).fetchone() is None: raise RequestError("Osoba neexistuje")
            for aid in ids:
                rows=self.faces_db.execute("SELECT id,state FROM faces WHERE asset_id=? AND state!='ignored' ORDER BY id",(aid,)).fetchall()
                if self.faces_db.execute("SELECT 1 FROM faces WHERE asset_id=? AND person_id=? AND state!='ignored' LIMIT 1",(aid,person)).fetchone():
                    self.faces_db.execute("UPDATE faces SET state='confirmed',score=NULL WHERE asset_id=? AND person_id=? AND state!='ignored'",(aid,person))
                else:
                    eligible=[r for r in rows if r[1] in ('unknown','auto','suggested')]
                    if len(eligible)==1:self.faces_db.execute("UPDATE faces SET person_id=?,state='confirmed',score=NULL WHERE id=?",(person,int(eligible[0][0])))
                    else:self.faces_db.execute("INSERT INTO faces(asset_id,x,y,w,h,det_score,source,ext_id,person_id,state,score,vector) VALUES(?,NULL,NULL,NULL,NULL,NULL,'ai',NULL,?,'confirmed',NULL,NULL)",(aid,person))
                changed+=1
            self.faces_db.commit();self._refresh_face_references_locked()
        return {"changed":changed}

    def _faces_asset_remove(self, request: dict[str, Any]) -> dict[str, Any]:
        ids=self._faces_asset_ids_arg(request);person=request.get("person_id")
        if isinstance(person,bool) or not isinstance(person,int) or person<=0: raise RequestError("person_id musí byť kladné ID")
        changed=0
        with self.faces_lock:
            for aid in ids:
                rows=self.faces_db.execute("SELECT id,x,y,w,h FROM faces WHERE asset_id=? AND person_id=?",(aid,person)).fetchall()
                if not rows:continue
                for fid,x,y,w,h in rows:
                    if any(v is None for v in (x,y,w,h)):
                        self.faces_db.execute("DELETE FROM face_thumbs WHERE face_id=?",(fid,));self.faces_db.execute("DELETE FROM face_rejected WHERE face_id=?",(fid,));self.faces_db.execute("DELETE FROM faces WHERE id=?",(fid,))
                    else:
                        self.faces_db.execute("INSERT OR IGNORE INTO face_rejected(face_id,person_id) VALUES(?,?)",(fid,person));self.faces_db.execute("UPDATE faces SET person_id=NULL,state='unknown',score=NULL WHERE id=?",(fid,))
                changed+=1
            self.faces_db.commit();self._refresh_face_references_locked()
        return {"changed":changed}

    def _faces_person_create(self, request: dict[str, Any]) -> dict[str, Any]:
        name = request.get("name")
        if not isinstance(name, str) or not 1 <= len(name.strip()) <= 100: raise RequestError("Meno musí mať 1 až 100 znakov")
        name = name.strip()
        with self.faces_lock:
            row = self.faces_db.execute("SELECT id FROM persons WHERE name=? COLLATE NOCASE", (name,)).fetchone()
            if row: return {"id": int(row[0])}
            cur = self.faces_db.execute("INSERT INTO persons(name,created) VALUES(?,?)", (name, time.time())); self.faces_db.commit()
            return {"id": int(cur.lastrowid)}

    def _faces_person_rename(self, request: dict[str, Any]) -> dict[str, Any]:
        person, name = request.get("person_id"), request.get("name")
        if isinstance(person, bool) or not isinstance(person, int) or person <= 0 or not isinstance(name, str) or not 1 <= len(name.strip()) <= 100: raise RequestError("Neplatné ID alebo meno")
        name = name.strip()
        with self.faces_lock:
            row = self.faces_db.execute("SELECT id FROM persons WHERE name=? COLLATE NOCASE AND id!=?", (name, person)).fetchone()
            if row: raise RequestError("Toto meno už používa iná osoba")
            cur = self.faces_db.execute("UPDATE persons SET name=? WHERE id=?", (name, person)); self.faces_db.commit()
            if not cur.rowcount: raise RequestError("Osoba neexistuje")
        return {"ok": True}

    def _faces_recognize_all(self) -> dict[str, Any]:
        if self.faces_db is None: return {"available": False}
        with self.faces_lock:
            ids = [int(r[0]) for r in self.faces_db.execute("SELECT id FROM faces WHERE vector IS NOT NULL AND state IN ('unknown','auto','suggested') ORDER BY id")]
        # Batch matrix multiplication against the single in-memory reference matrix.
        changed = 0
        for start in range(0, len(ids), 256): changed += self._recognize_face_ids(ids[start:start + 256])
        return {"changed": changed}

    def _face_files_present(self) -> bool:
        base = Path(self.model_dir).parent
        return (base / "faces" / "det_10g.onnx").is_file() and (base / "faces" / "w600k_r50.onnx").is_file() and (base / "pylibs").is_dir()

    def _faces_data_ready(self) -> bool:
        """Face data commands only read/write faces.sqlite and stored vectors; they must not
        wait for the SigLIP/ONNX models (loading them takes tens of seconds). Only scanning needs models."""
        if self.faces_db is None or self._faces_error or not (self._faces_available or self._face_files_present()):
            return False
        if not getattr(self, "_face_refs_ready", False):
            with self.faces_lock:
                if not getattr(self, "_face_refs_ready", False):
                    self._refresh_face_references_locked()
                    self._face_refs_ready = True
        return True

    def _faces_command(self, request: dict[str, Any]) -> dict[str, Any]:
        cmd = request.get("cmd")
        if not self._faces_data_ready():
            if cmd == "faces_status": return {"available": False, "scanned": 0, "pending": 0, "faces": 0, "named": 0, "unknown": 0, "persons": 0}
            return {"available": False}
        if cmd == "faces_status": return self._faces_status()
        if cmd == "faces_persons": return self._faces_persons()
        if cmd == "faces_person_assets": return self._faces_person_assets(request)
        if cmd == "faces_review": return self._faces_review(request)
        if cmd == "faces_clusters": return self._faces_clusters(request)
        if cmd == "faces_for_assets": return self._faces_for_assets(request)
        if cmd == "faces_set": return self._faces_set(request)
        if cmd == "faces_asset_assign": return self._faces_asset_assign(request)
        if cmd == "faces_asset_remove": return self._faces_asset_remove(request)
        if cmd == "faces_person_create": return self._faces_person_create(request)
        if cmd == "faces_person_rename": return self._faces_person_rename(request)
        if cmd == "faces_recognize_all": return self._faces_recognize_all()
        return {"available": False}
