import io
import json
import os
import tempfile
import threading
import time
import unittest
from pathlib import Path

import numpy as np

from ddreams_ai import AIService, DIMENSION, RequestError


class FakeEmbedder:
    dimension = DIMENSION
    device = "cpu"

    def __init__(self, model_dir):
        self.vectors = {}

    @staticmethod
    def vector(label):
        vector = np.zeros(DIMENSION, dtype=np.float32)
        if "east" in label:
            vector[0] = 1
        elif "west" in label:
            vector[1] = 1
        else:
            vector[2 + (sum(label.encode("utf-8")) % (DIMENSION - 2))] = 1
        return vector

    def encode_images(self, images):
        return np.stack([self.vector(str(image)) for image in images])

    def replace_index(self, ids, vectors):
        self.vectors = {int(a): v.copy() for a, v in zip(ids, vectors)}

    def upsert_index(self, ids, vectors):
        self.vectors.update({int(a): v.copy() for a, v in zip(ids, vectors)})

    def remove_index(self, ids):
        for aid in ids:
            self.vectors.pop(int(aid), None)

    def search_text(self, query, limit):
        target = self.vector(query)
        scores = sorted(((aid, float(np.dot(vector, target))) for aid, vector in self.vectors.items()), key=lambda pair: (-pair[1], pair[0]))
        return [{"asset_id": aid, "score": score} for aid, score in scores[:limit]]

    def similar(self, asset_id, limit):
        if asset_id not in self.vectors:
            raise RequestError("Fotka ešte nie je zaindexovaná")
        target = self.vectors[asset_id]
        scores = sorted(((aid, float(np.dot(vector, target))) for aid, vector in self.vectors.items() if aid != asset_id), key=lambda pair: (-pair[1], pair[0]))
        return [{"asset_id": aid, "score": score} for aid, score in scores[:limit]]


class ServerTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.db = self.root / "ai.sqlite"
        self.events = []
        self.next_id = 100
        self.service = AIService(self.db, "fake", embedder_factory=FakeEmbedder, decoder=lambda item: Path(item["path"]).name, output=self.events.append)
        self.service.start()
        self.assertTrue(self.service._model_ready.wait(5))
        self.assertIsNone(self.service._model_error)

    def tearDown(self):
        if self.service.db:
            self.service.stop()
        self.temp.cleanup()

    def make_file(self, name):
        path = self.root / name
        path.write_bytes(name.encode())
        stat = path.stat()
        item = {"asset_id": self.next_id, "path": str(path), "size": stat.st_size, "mtime_ns": stat.st_mtime_ns}
        self.next_id += 1
        return item

    def wait_idle(self, timeout=5):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            status = self.service.status()
            if status["queued"] == 0 and status["state"] != "indexing":
                return status
            time.sleep(0.02)
        self.fail("indexing did not become idle")

    def index(self, items, prune=False):
        return self.service.execute({"cmd": "index", "items": items, "prune": prune})

    def test_protocol_happy_path_and_search_ordering(self):
        east1 = self.make_file("east-1.jpg")
        east2 = self.make_file("east-2.jpg")
        west = self.make_file("west.jpg")
        self.index([east1, east2, west])
        self.wait_idle()
        result = self.service.execute({"cmd": "search_text", "query": "east", "limit": 3})
        self.assertEqual(sorted([east1["asset_id"], east2["asset_id"]]) + [west["asset_id"]], [row["asset_id"] for row in result])
        similar = self.service.execute({"cmd": "similar", "asset_id": east1["asset_id"]})
        self.assertNotIn(east1["asset_id"], [row["asset_id"] for row in similar])
        hello = self.service.execute({"cmd": "hello"})
        self.assertEqual((1, "ViT-L-16-SigLIP2-256", DIMENSION), (hello["protocol"], hello["model"], hello["dim"]))

    def test_unknown_command_and_bad_arguments(self):
        with self.assertRaisesRegex(RequestError, "Neznámy príkaz"):
            self.service.execute({"cmd": "not-a-command"})
        with self.assertRaisesRegex(RequestError, "query"):
            self.service.execute({"cmd": "search_text", "query": "  "})
        with self.assertRaisesRegex(RequestError, "asset_id"):
            self.service.execute({"cmd": "similar", "asset_id": -1})

    def test_index_diff_changed_mtime_and_prune(self):
        first, second = self.make_file("one-east.jpg"), self.make_file("two-west.jpg")
        self.index([first, second])
        self.wait_idle()
        self.assertEqual(self.index([first, second]), {"queued": 0, "up_to_date": 2, "failed_before": 0, "pruned": 0})
        changed = dict(first, mtime_ns=first["mtime_ns"] + 1)
        self.assertEqual(self.index([changed], prune=True), {"queued": 1, "up_to_date": 0, "failed_before": 0, "pruned": 1})
        self.wait_idle()
        self.assertEqual(self.service.indexed_count, 1)

    def test_failures_cached_until_metadata_changes_and_offline_retry(self):
        bad = self.make_file("bad.jpg")
        offline = {"asset_id": 987654321, "path": str(self.root / "missing.jpg"), "size": 4, "mtime_ns": 5}
        self.service.decoder = lambda item: (_ for _ in ()).throw(ValueError("decode broke")) if Path(item["path"]).name == "bad.jpg" else Path(item["path"]).name
        self.index([bad, offline])
        self.wait_idle()
        self.assertEqual(self.service.status()["failed"], 1)
        self.assertEqual(self.service.status()["offline"], 1)
        self.assertEqual(self.service.db.execute("SELECT count(*) FROM failures").fetchone()[0], 1)
        self.assertEqual(self.index([bad, offline])["failed_before"], 1)
        changed = dict(bad, mtime_ns=bad["mtime_ns"] + 1)
        self.assertEqual(self.index([changed])["queued"], 1)

    def test_replacing_queue_discards_inflight_batch_and_returns_to_ready(self):
        first, second = self.make_file("ready-east.jpg"), self.make_file("slow-west.jpg")
        self.index([first])
        self.wait_idle()
        entered, release = threading.Event(), threading.Event()
        def slow_decoder(item):
            if item["asset_id"] == second["asset_id"]:
                entered.set()
                release.wait(5)
            return Path(item["path"]).name
        self.service.decoder = slow_decoder
        self.index([second])
        self.assertTrue(entered.wait(5))
        self.assertEqual(self.index([first], prune=True)["up_to_date"], 1)
        release.set()
        self.wait_idle()
        self.assertEqual(self.service.status()["state"], "ready")
        self.assertEqual(self.service.db.execute("SELECT count(*) FROM embeddings").fetchone()[0], 1)

    def test_similar_unindexed_and_shutdown_on_eof(self):
        with self.assertRaisesRegex(RequestError, "Fotka ešte nie je zaindexovaná"):
            self.service.execute({"cmd": "similar", "asset_id": 123})
        path = self.root / "eof.sqlite"
        service = AIService(path, "fake", embedder_factory=FakeEmbedder, decoder=lambda item: item["path"])
        output = io.StringIO()
        service.serve(io.StringIO('{"id":1,"cmd":"hello"}\n'), output)
        messages = [json.loads(line) for line in output.getvalue().splitlines()]
        self.assertTrue(any(msg.get("id") == 1 and msg.get("ok") for msg in messages))
        self.assertTrue(service._stop)

    def test_jsonl_shutdown_and_error_isolation(self):
        service = AIService(self.root / "wire.sqlite", "fake", embedder_factory=FakeEmbedder, decoder=lambda item: item["path"])
        output = io.StringIO()
        service.serve(io.StringIO('{"id":1,"cmd":"bad"}\n{"id":2,"cmd":"shutdown"}\n'), output)
        messages = [json.loads(line) for line in output.getvalue().splitlines()]
        self.assertTrue(any(m.get("id") == 1 and m.get("ok") is False for m in messages))
        self.assertTrue(any(m.get("id") == 2 and m.get("ok") is True for m in messages))


if __name__ == "__main__":
    unittest.main()
