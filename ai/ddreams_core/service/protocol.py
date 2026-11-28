from __future__ import annotations

import json
import sys
import threading
from typing import Any
import numpy as np

from ..constants import DIMENSION, SIMILARITY_COSINE_FLOOR
from ..util import RequestError


class ProtocolMixin:
    def execute(self, request: dict[str, Any]) -> dict[str, Any]:
        cmd = request.get("cmd")
        if cmd == "hello":
            return {"protocol": 1, "model": self.model_name, "dim": DIMENSION, "device": self._device,
                    "state": self.status()["state"], "indexed": self.indexed_count}
        if cmd == "status":
            return self.status()
        if cmd == "load_model":
            self._start_model()
            return self.status()
        if cmd == "index":
            return self._index_command(request)
        if cmd == "light_sync":
            return self._light_sync(request)
        if cmd == "forget":
            return self._forget_command(request)
        if cmd in {"faces_status", "faces_persons", "faces_person_assets", "faces_review", "faces_clusters", "faces_for_assets", "faces_set", "faces_asset_assign", "faces_asset_remove", "faces_person_create", "faces_person_rename", "faces_recognize_all"}:
            return self._faces_command(request)
        if cmd == "learn_colors":
            return self._learn_colors(request)
        if cmd == "set_labels":
            return self._set_labels(request)
        if cmd == "suggest":
            return self._suggest(request)
        if cmd == "content_keywords":
            return self._content_keywords(request)
        if cmd == "suggest_batch":
            return self._suggest_batch(request)
        if cmd == "map_build":
            return self._map_build()
        if cmd == "duplicate_groups":
            return self._duplicate_groups(request)
        if cmd == "clusters":
            return self._clusters(request)
        if cmd == "cluster_ignore":
            return self._cluster_ignore(request)
        if cmd == "cluster_ignore_reset":
            return self._cluster_ignore_reset(request)
        if cmd == "indexed_asset_ids":
            return self._indexed_asset_ids(request)
        if cmd == "suggest_folders":
            return self._suggest_folders(request)
        if cmd == "keyword_matches":
            return self._keyword_matches(request)
        if cmd == "incoming_groups":
            return self._incoming_groups(request)
        if cmd == "series":
            return self._series(request)
        if cmd == "order_by_similarity":
            return self._order_by_similarity(request)
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
            self._start_model()
            embedder = self._wait_model()
            index = self._index_snapshot(use_cuda=str(getattr(embedder, "device", "")).startswith("cuda"))
            return embedder.search_text(query.strip(), limit, index)
        if cmd == "similar":
            aid = request.get("asset_id")
            if isinstance(aid, bool) or not isinstance(aid, int) or aid <= 0:
                raise RequestError("asset_id musí byť kladné celé číslo")
            limit = self._limit(request.get("limit", 200))
            embedder = self._embedder
            index = self._index_snapshot(use_cuda=str(getattr(embedder, "device", "")).startswith("cuda"))
            ids, matrix, positions = index
            if aid not in positions: raise RequestError("Fotka ešte nie je zaindexovaná")
            if not isinstance(matrix, np.ndarray):
                return embedder.similar(aid, limit, index)
            query_vector = matrix[positions[aid]].astype(np.float32)
            chunks = np.array_split(matrix, 2)
            futures = [self._vector_pool.submit(np.einsum, "ij,j->i", chunk, query_vector,
                                                dtype=np.float32, optimize=False) for chunk in chunks]
            values = np.concatenate([future.result() for future in futures])
            values = values.astype(np.float16).astype(np.float32)
            values[positions[aid]] = -2
            take = min(limit, max(0, len(ids) - 1))
            chosen = np.argpartition(-values, take - 1)[:take] if take else np.empty(0, dtype=np.int64)
            chosen = sorted(chosen, key=lambda i: (-values[i], ids[i]))
            results = []
            for i in chosen:
                other, score = ids[int(i)], float(values[i])
                if score < SIMILARITY_COSINE_FLOOR: continue
                result = {"asset_id": other, "score": score}
                probability = self._similar_probability(score)
                if probability is not None: result["prob"] = probability
                results.append(result)
            return {"results": results, "searched": len(ids)}
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
                    if request.get("cmd") in {"search_text", "similar", "suggest", "content_keywords", "keyword_matches", "incoming_groups", "suggest_batch", "duplicate_groups", "map_build", "series", "suggest_folders", "clusters", "order_by_similarity", "faces_status", "faces_persons", "faces_person_assets", "faces_review", "faces_clusters", "faces_for_assets", "faces_set", "faces_asset_assign", "faces_asset_remove", "faces_person_create", "faces_person_rename", "faces_recognize_all"}:
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
        with self._condition:
            self._stop = True
            self._condition.notify_all()
        self._query_pool.shutdown(wait=True)
        self._vector_pool.shutdown(wait=True)
        if self._folder_catalog is not None:
            self._folder_catalog.close()
        if self._loader and self._loader is not threading.current_thread():
            self._loader.join()
        if self._worker and self._worker is not threading.current_thread():
            self._worker.join()
        if self._face_worker and self._face_worker is not threading.current_thread():
            self._face_worker.join()
        if self.faces_db is not None:
            with self.faces_lock:
                self.faces_db.commit()
                self.faces_db.close()
        with self.db_lock:
            self.db.commit()
            self.db.close()
