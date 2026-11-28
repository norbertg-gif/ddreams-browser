from __future__ import annotations

import concurrent.futures
from collections import deque
import gc
import json
import os
import sqlite3
import threading
import time
import traceback
from pathlib import Path
from typing import Any, Callable
import numpy as np
from faces_schema import ensure_faces_schema

from ..constants import MODEL_NAME, SCHEMA_VERSION, BATCH_SIZE
from ..colors import learn_thresholds
from ..util import RequestError, _stderr, process_memory
from ..decode import decode_photo
from ..embedder import SiglipEmbedder
from ..vector_index import VectorIndex


class BaseMixin:
    def __init__(self, db_path: str | os.PathLike[str], model_dir: str | os.PathLike[str], *,
                 embedder_factory: Callable[[str | os.PathLike[str]], Any] | None = None,
                 decoder: Callable[[dict[str, Any]], Any] = decode_photo,
                 output: Callable[[dict[str, Any]], None] | None = None,
                 face_detector: Any = None, face_encoder: Any = None, lazy_model: bool = False):
        self.db_path = str(db_path)
        self.model_dir = str(model_dir)
        self.model_name = MODEL_NAME
        self.embedder_factory = embedder_factory or SiglipEmbedder
        self.decoder = decoder
        self.output = output or (lambda obj: None)
        self.db = sqlite3.connect(self.db_path, check_same_thread=False)
        self.db.execute("PRAGMA journal_mode=WAL")
        self.db.execute("PRAGMA synchronous=NORMAL")
        self.faces_db_path = str(Path(self.db_path).with_name("faces.sqlite"))
        self.faces_lock = threading.RLock()
        self.faces_db: sqlite3.Connection | None = None
        self._faces_available = False
        self._faces_error: str | None = None
        self._face_model_lock = threading.Lock()
        self._face_detector = face_detector
        self._face_encoder = face_encoder
        self._face_det_input = None
        self._face_encoder_input = None
        if face_detector is not None and face_encoder is not None:
            self._faces_available = True
        try:
            self.faces_db = sqlite3.connect(self.faces_db_path, check_same_thread=False)
            self.faces_db.execute("PRAGMA synchronous=NORMAL")
            ensure_faces_schema(self.faces_db)
        except Exception as exc:
            self._faces_error = str(exc)
            self.faces_db = None
        self.db.executescript("""
            CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS embeddings(asset_id INTEGER PRIMARY KEY, model TEXT NOT NULL, size INTEGER NOT NULL, mtime_ns INTEGER NOT NULL, dim INTEGER NOT NULL, vector BLOB NOT NULL);
            CREATE TABLE IF NOT EXISTS failures(asset_id INTEGER PRIMARY KEY, size INTEGER NOT NULL, mtime_ns INTEGER NOT NULL, error TEXT NOT NULL, at REAL NOT NULL);
            CREATE TABLE IF NOT EXISTS colors(asset_id INTEGER PRIMARY KEY, size INTEGER NOT NULL, mtime_ns INTEGER NOT NULL, shares TEXT NOT NULL, auto TEXT NOT NULL, version INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS quality(asset_id INTEGER PRIMARY KEY, size INTEGER, mtime_ns INTEGER, sharp REAL, hi REAL, lo REAL, mean REAL, version INTEGER);
            CREATE TABLE IF NOT EXISTS cluster_ignored(id INTEGER PRIMARY KEY, vector BLOB NOT NULL, created REAL NOT NULL);
            CREATE TABLE IF NOT EXISTS map_points(asset_id INTEGER PRIMARY KEY, x REAL NOT NULL, y REAL NOT NULL, cluster INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS map_clusters(id INTEGER PRIMARY KEY, x REAL NOT NULL, y REAL NOT NULL, r REAL NOT NULL, count INTEGER NOT NULL, name TEXT);
            CREATE TABLE IF NOT EXISTS text_vectors(model TEXT NOT NULL, text TEXT NOT NULL, vector BLOB NOT NULL, PRIMARY KEY(model,text));

        """)
        self.db_lock = threading.RLock()
        self._lock = threading.RLock()
        self._retry_decode_failures()
        meta = dict(self.db.execute("SELECT key,value FROM meta").fetchall())
        if meta.get("model") != self.model_name or meta.get("schema_version") != SCHEMA_VERSION:
            self.db.execute("DELETE FROM embeddings")
            self.db.execute("DELETE FROM failures")
            self.db.execute("INSERT OR REPLACE INTO meta(key,value) VALUES('model',?)", (self.model_name,))
            self.db.execute("INSERT OR REPLACE INTO meta(key,value) VALUES('schema_version',?)", (SCHEMA_VERSION,))
        color_meta = meta.get("color_thresholds")
        if color_meta:
            self.color_thresholds = json.loads(color_meta)
        else:
            self.color_thresholds = learn_thresholds({})
            self.db.execute("INSERT OR REPLACE INTO meta(key,value) VALUES('color_thresholds',?)", (json.dumps(self.color_thresholds, ensure_ascii=False),))
        self.db.commit()
        self.indexed_count = int(self.db.execute("SELECT count(*) FROM embeddings WHERE model=?", (self.model_name,)).fetchone()[0])
        self._condition = threading.Condition()
        self._model_ready = threading.Event()
        self._lazy_model = lazy_model
        self._model_started = False
        self._vectors: VectorIndex | None = None
        self._cuda_torch = None
        self._vector_build_lock = threading.Lock()
        self._vector_cuda_lock = threading.Lock()
        self._vector_upload_lock = threading.Lock()
        self._folder_cuda_pending = False
        self._folder_catalog_lock = threading.Lock()
        self._folder_catalog = None
        self._folder_catalog_path = None
        self._folder_metadata_key = None
        self._folder_metadata = {}
        self._similar_logits: tuple[float, float] | None = None
        self._similar_logits_checked = False
        self._model_error: str | None = None
        self._embedder: Any = None
        self._stop = False
        self._paused = False
        self._queue: list[dict[str, Any]] = []
        self._quality_queue: list[dict[str, Any]] = []
        self._face_scan_queue: deque[dict[str, Any]] = deque()
        self._face_active_ids: set[int] = set()
        self._newly_indexed_ids: set[int] = set()
        self._last_face_progress = 0.0
        self._face_refs_ids = np.empty(0, dtype=np.int64)
        self._face_refs_people = np.empty(0, dtype=np.int64)
        self._face_refs_matrix = np.empty((0, 512), dtype=np.float32)
        self._generation = 0
        self._active = False
        self._done = self._failed = self._offline = 0
        self._total = 0
        self._started_at: float | None = None
        self._last_error: str | None = None
        self._last_progress = 0.0
        self._model_load_s: float | None = None
        self._device = "cuda" if embedder_factory is None else "cpu"
        self._labels: dict[str, dict[int, set[int]]] = {"category": {}, "keyword": {}}
        self._parents: dict[str, dict[int, int | None]] = {"category": {}, "keyword": {}}
        self._cluster_text_vectors: dict[str, np.ndarray] | None = None
        self._keyword_lock = threading.Lock()
        self._keyword_matrix: np.ndarray | None = None
        self._query_pool = concurrent.futures.ThreadPoolExecutor(max_workers=2, thread_name_prefix="ai-query")
        self._vector_pool = concurrent.futures.ThreadPoolExecutor(max_workers=2, thread_name_prefix="ai-vector")
        self._loader: threading.Thread | None = None
        self._worker: threading.Thread | None = None
        self._face_worker: threading.Thread | None = None

    def _emit(self, obj: dict[str, Any]) -> None:
        try:
            self.output(obj)
        except Exception:
            _stderr("Nepodarilo sa odoslať protokolovú správu")

    def start(self) -> None:
        self._worker = threading.Thread(target=self._index_loop, name="ai-indexer", daemon=True)
        self._face_worker = threading.Thread(target=self._face_scan_loop, name="ai-face-scan", daemon=True)
        if not self._lazy_model:
            self._start_model()
        else:
            self._emit({"event": "status", **self.status()})
        self._worker.start()
        self._face_worker.start()

    def _start_model(self) -> None:
        with self._condition:
            if self._model_started or self._stop:
                return
            self._model_started = True
            self._loader = threading.Thread(target=self._load_model, name="ai-model-loader", daemon=True)
            self._loader.start()
        self._emit({"event": "status", **self.status()})

    def _load_model(self) -> None:
        started = time.perf_counter()
        try:
            _stderr("Načítavam lokálny SigLIP2 model…")
            embedder = self.embedder_factory(self.model_dir)
            self._device = str(getattr(embedder, "device", self._device))
            # Reuse the lazy-mode device index if it already exists.
            self._index_snapshot(use_cuda=self._device.startswith("cuda"))
            with self.db_lock:
                self._embedder = embedder
            gc.collect()
            self._model_ready.set()
            self._model_load_s = round(time.perf_counter() - started, 3)
            ready_status = self.status()
            ready_status["state"] = "ready"
            ready_status["model_load_s"] = self._model_load_s
            self._emit({"event": "status", **ready_status})
            if hasattr(embedder, "torch"):
                memory = process_memory()
                if memory:
                    _stderr(f"MEMORY ready working_set={memory['working_set']} private={memory['private']}")
                if str(getattr(embedder, "device", "")).startswith("cuda"):
                    _stderr(f"CUDA ready allocated={embedder.torch.cuda.memory_allocated()} reserved={embedder.torch.cuda.memory_reserved()}")
            _stderr(f"SigLIP2 pripravený za {self._model_load_s:.3f} s")
        except Exception as exc:
            text = str(exc)
            if "1455" in text or "paging file" in text.lower() or "invalid python storage" in text:
                self._model_error = "Málo voľnej pamäte na načítanie modelu. Zavri náročné programy (napr. iný AI nástroj) a v paneli AI daj Vypnúť a Zapnúť AI."
            else:
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
                state = "ready" if self._lazy_model else "error"
            elif self._paused:
                state = "paused"
            elif self._model_started and not self._model_ready.is_set() and not self._lazy_model:
                state = "loading"
            elif self._model_ready.is_set() and (self._active or self._queue or self._quality_queue):
                state = "indexing"
            else:
                state = "ready"
            queued = len(self._queue) + len(self._quality_queue)
            elapsed = max(0.001, time.monotonic() - self._started_at) if self._started_at else 0.0
            processed = self._done + self._failed + self._offline
            rate = processed / elapsed if elapsed else 0.0
            eta = (queued + (BATCH_SIZE if self._active else 0)) / rate if rate else None
            faces = {"available": bool(self._faces_available), "scanned": 0, "pending": len(self._face_scan_queue) + len(self._face_active_ids)}
            if self.faces_db is not None and self._faces_available:
                try:
                    with self.faces_lock:
                        faces["scanned"] = int(self.faces_db.execute("SELECT count(*) FROM face_scan").fetchone()[0])
                except sqlite3.Error:
                    pass
            model_state = "error" if self._model_error else "ready" if self._model_ready.is_set() else "loading" if self._model_started else "unloaded"
            waiting = len({item["asset_id"] for item in self._queue + self._quality_queue} | {item["asset_id"] for item in self._face_scan_queue} | self._face_active_ids)
            return {"state": state, "model_state": model_state, "waiting": waiting, "indexed": self.indexed_count, "queued": queued, "done": self._done,
                    "failed": self._failed, "offline": self._offline, "rate_per_s": round(rate, 3),
                    "eta_s": round(eta, 1) if eta is not None else None, "last_error": self._last_error, "faces": faces}

    def _wait_model(self) -> Any:
        self._model_ready.wait()
        if self._model_error:
            raise RequestError(self._model_error)
        if self._stop:
            raise RequestError("Server sa vypína")
        return self._embedder
