import io
import json
import os
import tempfile
import threading
import time
import unittest
from pathlib import Path

import numpy as np
from PIL import Image, ImageFilter

from ddreams_ai import AIService, COLOR_PALETTE, DIMENSION, RequestError, compute_quality, dominant_colors, learn_thresholds, palette_counts, text_probability_cutoff


class FakePhoto:
    def __init__(self, name): self.name = name
    def __str__(self): return self.name
    def convert(self, mode): return Image.new(mode, (16, 16), "white")


class QualityTests(unittest.TestCase):
    def test_sharp_checkerboard_scores_above_blurred_copy(self):
        pixels = (np.indices((128, 128)).sum(axis=0) % 2 * 255).astype(np.uint8)
        checker = Image.fromarray(pixels, mode="L").convert("RGB")
        sharp = compute_quality(checker)[0]
        blurred = compute_quality(checker.filter(ImageFilter.GaussianBlur(3)))[0]
        self.assertGreater(sharp, blurred * 5)

    def test_clipped_highlights_and_shadows(self):
        self.assertEqual(compute_quality(Image.new("RGB", (16, 16), "white"))[1], 1.0)
        _, _, lo, mean = compute_quality(Image.new("RGB", (16, 16), "black"))
        self.assertEqual(lo, 1.0)
        self.assertEqual(mean, 0.0)


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
        return {"results": [{"asset_id": aid, "score": score, "prob": 0.9 if score > 0 else 0.01} for aid, score in scores[:limit] if score >= 0.2], "searched": len(self.vectors)}

    def similar(self, asset_id, limit):
        if asset_id not in self.vectors:
            raise RequestError("Fotka ešte nie je zaindexovaná")
        target = self.vectors[asset_id]
        scores = sorted(((aid, float(np.dot(vector, target))) for aid, vector in self.vectors.items() if aid != asset_id), key=lambda pair: (-pair[1], pair[0]))
        return {"results": [{"asset_id": aid, "score": score, "prob": 0.9 if score > 0 else 0.01} for aid, score in scores[:limit] if score >= 0.6], "searched": len(self.vectors)}


class ServerTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.db = self.root / "ai.sqlite"
        self.events = []
        self.next_id = 100
        self.service = AIService(self.db, "fake", embedder_factory=FakeEmbedder, decoder=lambda item: FakePhoto(Path(item["path"]).name), output=self.events.append)
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
        self.assertEqual({east1["asset_id"], east2["asset_id"]}, {row["asset_id"] for row in result["results"]})
        self.assertEqual(3, result["searched"])
        similar = self.service.execute({"cmd": "similar", "asset_id": east1["asset_id"]})
        self.assertNotIn(east1["asset_id"], [row["asset_id"] for row in similar["results"]])
        hello = self.service.execute({"cmd": "hello"})
        self.assertEqual((1, "ViT-L-16-SigLIP2-256", DIMENSION), (hello["protocol"], hello["model"], hello["dim"]))

    def test_quality_is_indexed_and_backfilled_at_low_priority(self):
        item = self.make_file("quality-backfill.jpg")
        self.index([item]); self.wait_idle()
        row = self.service.db.execute("SELECT size,mtime_ns,sharp,hi,lo,mean,version FROM quality WHERE asset_id=?", (item["asset_id"],)).fetchone()
        self.assertIsNotNone(row)
        self.assertEqual((item["size"], item["mtime_ns"], 1), (row[0], row[1], row[6]))
        self.service.db.execute("DELETE FROM quality WHERE asset_id=?", (item["asset_id"],)); self.service.db.commit()
        self.assertEqual(self.index([item])["queued"], 0)
        self.assertEqual([queued["asset_id"] for queued in self.service._quality_queue], [item["asset_id"]])
        self.wait_idle()
        self.assertEqual(self.service.db.execute("SELECT count(*) FROM quality WHERE asset_id=?", (item["asset_id"],)).fetchone()[0], 1)

    def series(self, timed_ids, vectors, sharp=None, max_gap=10, min_sim=0.9):
        self.service._embedder.vectors.update(vectors)
        for asset_id, value in (sharp or {}).items():
            self.service.db.execute("INSERT OR REPLACE INTO quality(asset_id,sharp) VALUES(?,?)", (asset_id, value))
        self.service.db.commit()
        return self.service.execute({"cmd": "series", "items": [{"asset_id": aid, "t": stamp} for aid, stamp in timed_ids],
                                     "max_gap": max_gap, "min_sim": min_sim})["groups"]

    def test_series_chain_and_sharpness_tie_uses_earliest_time(self):
        vectors = {10: np.array([1, 0], dtype=np.float32), 11: np.array([0.98, 0.2], dtype=np.float32), 12: np.array([0.96, 0.28], dtype=np.float32)}
        groups = self.series([(10, 1.0), (11, 2.0), (12, 3.0)], vectors, {10: 12.0, 11: 25.0, 12: 25.0})
        self.assertEqual(groups[0]["ids"], [10, 11, 12])
        self.assertEqual(groups[0]["best"], 11)

    def test_series_gap_breaks_chain(self):
        vector = np.array([1, 0], dtype=np.float32)
        groups = self.series([(20, 1.0), (21, 2.0), (22, 20.0)], {20: vector, 21: vector, 22: vector})
        self.assertEqual([group["ids"] for group in groups], [[20, 21]])

    def test_series_similarity_breaks_chain(self):
        groups = self.series([(30, 1.0), (31, 2.0), (32, 3.0)], {
            30: np.array([1, 0], dtype=np.float32), 31: np.array([1, 0], dtype=np.float32),
            32: np.array([0, 1], dtype=np.float32),
        })
        self.assertEqual([group["ids"] for group in groups], [[30, 31]])

    def test_missing_embedding_breaks_chain(self):
        vector = np.array([1, 0], dtype=np.float32)
        groups = self.series([(40, 1.0), (41, 2.0), (42, 3.0)], {40: vector, 42: vector})
        self.assertEqual(groups, [])

    def test_search_cutoff_and_decode_only_color_pass(self):
        item = self.make_file("east-colors.jpg")
        self.index([item]); self.wait_idle()
        self.assertEqual(self.index([item])['queued'], 0)
        self.service.db.execute("DELETE FROM colors WHERE asset_id=?", (item["asset_id"],)); self.service.db.commit()
        self.assertEqual(self.index([item])['queued'], 1)
        queued = self.service._queue[0]
        self.assertTrue(queued["colors_only"])
        self.service._queue.clear()
        embedder = self.service._embedder
        embedder.vectors.clear()
        self.assertEqual(self.service.execute({"cmd": "search_text", "query": "unrelated", "limit": 5})["results"], [])

    def test_relative_text_cutoff_keeps_low_probability_true_matches(self):
        self.assertAlmostEqual(text_probability_cutoff(0.39), 0.0195)
        self.assertAlmostEqual(text_probability_cutoff(0.015), 0.003)
        probs = [0.39, 0.16, 0.074, 0.015, 0.006, 0.0006]
        cutoff = text_probability_cutoff(max(probs))
        self.assertEqual([p for p in probs if p >= cutoff], [0.39, 0.16, 0.074])
        floor_cutoff = text_probability_cutoff(0.015)
        self.assertEqual([p for p in probs if p >= floor_cutoff], [0.39, 0.16, 0.074, 0.015, 0.006])

    def test_color_algorithm_matches_photosort_synthetic(self):
        import sys
        photosort = Path(r"G:\Programovanie\photosort")
        if not (photosort / "photosort/colors.py").exists(): self.skipTest("photosort source missing")
        sys.path.insert(0, str(photosort))
        from photosort.colors import dominant_colors as reference
        for rgb in ((220, 20, 30), (130, 130, 130), (255, 255, 255), (0, 0, 0)):
            image = Image.new("RGB", (12, 10), rgb)
            expected, shares = reference(image, return_shares=True)
            ours = palette_counts(image)
            self.assertEqual(expected, dominant_colors(ours))
            self.assertEqual(set(shares), set(ours))
            for key in shares: self.assertAlmostEqual(shares[key], ours[key], places=7)

    def test_learned_thresholds_match_photosort_corrections(self):
        import sys
        root = Path(r"G:\Programovanie\photosort")
        corrections_path = root / "learned/color_corrections.json"
        if not corrections_path.exists(): self.skipTest("photosort learned corrections missing")
        sys.path.insert(0, str(root))
        from photosort.colors import learn_color_thresholds as reference
        corrections = json.loads(corrections_path.read_text(encoding="utf-8"))
        samples = {color: [(float(row.get("shares", {}).get(color, 0)), color in row.get("colors", []))
                           for row in corrections.values() if isinstance(row, dict)] for color in COLOR_PALETTE}
        expected = reference(corrections, path=self.root / "reference.json")
        actual = learn_thresholds(samples)
        self.assertEqual(set(expected), set(actual))
        for color in expected:
            self.assertAlmostEqual(expected[color]["threshold"], actual[color]["threshold"], places=12)
            self.assertEqual(expected[color]["examples"], actual[color]["examples"])

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

    def test_append_preserves_queue_and_deduplicates(self):
        first, second = self.make_file("append-one.jpg"), self.make_file("append-two.jpg")
        self.service.execute({"cmd": "pause"})
        self.index([first])
        generation = self.service._generation
        result = self.service.execute({"cmd": "index", "items": [first, second], "append": True})
        self.assertEqual(result["queued"], 1)
        self.assertEqual(self.service._generation, generation)
        self.assertEqual({item["asset_id"] for item in self.service._queue}, {first["asset_id"], second["asset_id"]})
        self.service.execute({"cmd": "resume"})
        self.wait_idle()
        self.assertEqual(self.service.indexed_count, 2)

    def test_forget_drops_rows_index_and_queue(self):
        first, second, third = self.make_file("forget-one.jpg"), self.make_file("forget-two.jpg"), self.make_file("forget-three.jpg")
        self.index([first, second])
        self.wait_idle()
        self.service.execute({"cmd": "pause"})
        self.service.execute({"cmd": "index", "items": [third], "append": True})
        result = self.service.execute({"cmd": "forget", "asset_ids": [first["asset_id"], third["asset_id"]]})
        self.assertEqual(result, {"forgotten": 1})
        self.assertEqual(self.service.indexed_count, 1)
        self.assertEqual(self.service._queue, [])
        ids = [r["asset_id"] for r in self.service.execute({"cmd": "similar", "asset_id": second["asset_id"], "limit": 10})["results"]]
        self.assertNotIn(first["asset_id"], ids)
        with self.assertRaises(RequestError):
            self.service.execute({"cmd": "forget", "asset_ids": "x"})

    def test_append_rejects_prune_and_non_boolean(self):
        with self.assertRaises(RequestError):
            self.service.execute({"cmd": "index", "items": [], "append": True, "prune": True})
        with self.assertRaises(RequestError):
            self.service.execute({"cmd": "index", "items": [], "append": 1})

    def test_append_keeps_inflight_batch(self):
        first, second = self.make_file("inflight-east.jpg"), self.make_file("appended-west.jpg")
        entered, release = threading.Event(), threading.Event()
        def slow_decoder(item):
            if item["asset_id"] == first["asset_id"]:
                entered.set()
                release.wait(5)
            return FakePhoto(Path(item["path"]).name)
        self.service.decoder = slow_decoder
        self.index([first])
        try:
            self.assertTrue(entered.wait(5))
            self.service.execute({"cmd": "index", "items": [second], "append": True})
        finally:
            release.set()
        self.wait_idle()
        self.assertEqual(set(self.service._embedder.vectors), {first["asset_id"], second["asset_id"]})

    def test_failures_cached_until_metadata_changes_and_offline_retry(self):
        bad = self.make_file("bad.jpg")
        offline = {"asset_id": 987654321, "path": str(self.root / "missing.jpg"), "size": 4, "mtime_ns": 5}
        self.service.decoder = lambda item: (_ for _ in ()).throw(ValueError("decode broke")) if Path(item["path"]).name == "bad.jpg" else FakePhoto(Path(item["path"]).name)
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
            return FakePhoto(Path(item["path"]).name)
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


    def test_label_suggestions_exclude_self_existing_and_ancestors(self):
        items = [self.make_file(f"knn-{name}.jpg") for name in ("query", "n1", "n2", "n3", "unlabeled")]
        self.index(items); self.wait_idle()
        vectors = self.service._embedder.vectors
        ids = [item["asset_id"] for item in items]
        vectors[ids[0]] = np.array([1.0, 0.0] + [0.0] * (DIMENSION - 2), dtype=np.float32)
        vectors[ids[1]] = np.array([0.9, (1 - 0.9 ** 2) ** 0.5] + [0.0] * (DIMENSION - 2), dtype=np.float32)
        vectors[ids[2]] = np.array([0.8, 0.6] + [0.0] * (DIMENSION - 2), dtype=np.float32)
        vectors[ids[3]] = np.array([0.7, (1 - 0.7 ** 2) ** 0.5] + [0.0] * (DIMENSION - 2), dtype=np.float32)
        vectors[ids[4]] = np.array([0.95, (1 - 0.95 ** 2) ** 0.5] + [0.0] * (DIMENSION - 2), dtype=np.float32)
        set_result = self.service.execute({"cmd": "set_labels", "kind": "category",
            "pairs": [[ids[0], 30], [ids[1], 20], [ids[1], 40], [ids[2], 50], [ids[3], 30], [ids[3], 40]],
            "parents": {"30": 20, "20": 10, "10": None, "40": None, "50": None}})
        self.assertEqual(set_result, {"assets": 4, "assignments": 6, "labels": 5})
        result = self.service.execute({"cmd": "suggest", "kind": "category", "asset_ids": [ids[0], ids[4]], "limit": 5})
        rows = result["results"][str(ids[0])]
        self.assertNotIn(ids[0], result["results"])
        self.assertEqual([40, 50], [row["label_id"] for row in rows])
        self.assertAlmostEqual(2.0 / 3.0, rows[0]["score"], places=5)
        self.assertEqual(2, rows[0]["support"])
        self.assertNotIn(30, [row["label_id"] for row in rows])
        self.assertNotIn(20, [row["label_id"] for row in rows])
        self.assertEqual(2, result["combined"][0]["count"])

    def test_suggest_batch_grouping_unlabeled_and_unknown_kind(self):
        items = [self.make_file(f"batch-{name}.jpg") for name in ("a", "b", "c", "d")]
        self.index(items); self.wait_idle()
        ids = [item["asset_id"] for item in items]
        vectors = self.service._embedder.vectors
        for aid, x in zip(ids, (1.0, 0.95, 0.9, 0.85)):
            vectors[aid] = np.array([x, (1 - x * x) ** 0.5] + [0.0] * (DIMENSION - 2), dtype=np.float32)
        self.service.execute({"cmd": "set_labels", "kind": "keyword", "pairs": [[ids[0], 7], [ids[1], 7]], "parents": {"7": None}})
        groups = self.service.execute({"cmd": "suggest_batch", "kind": "keyword", "asset_ids": "unlabeled", "min_score": 0.3})["groups"]
        self.assertEqual([ids[2], ids[3]], groups[0]["asset_ids"])
        with self.assertRaisesRegex(RequestError, "kind"):
            self.service.execute({"cmd": "suggest", "kind": "unknown", "asset_ids": [ids[0]]})

    def test_duplicate_groups_respect_threshold_without_chaining(self):
        # Seed 101 has two direct neighbours; 103 is close only to 102.
        vectors = np.zeros((4, DIMENSION), dtype=np.float32)
        vectors[0, :2] = [1, 0]
        vectors[1, :2] = [.9, .4358899]
        vectors[2, :2] = [.65, .75]
        vectors[3, :2] = [.9, -.4358899]
        vectors /= np.linalg.norm(vectors, axis=1, keepdims=True)
        self.service._embedder.replace_index([101, 102, 103, 104], vectors)
        groups = self.service.execute({"cmd": "duplicate_groups", "threshold": .85})["groups"]
        self.assertEqual(groups[0]["asset_ids"], [101, 102, 104])
        self.assertTrue(all(len(group["asset_ids"]) >= 2 for group in groups))
        self.assertTrue(all(group["min_sim"] >= .85 for group in groups))

    def test_order_by_similarity_neighbours_and_unindexed_last(self):
        vectors = np.zeros((3, DIMENSION), dtype=np.float32)
        vectors[0, 0] = 1
        vectors[1, :2] = [.99, .141067]
        vectors[2, 1] = 1
        vectors /= np.linalg.norm(vectors, axis=1, keepdims=True)
        self.service._embedder.replace_index([201, 202, 203], vectors)
        result = self.service.execute({"cmd": "order_by_similarity", "asset_ids": [201, 999, 203, 202]})["asset_ids"]
        self.assertEqual(result[-1], 999)
        self.assertLessEqual(abs(result.index(201) - result.index(202)), 1)


if __name__ == "__main__":
    unittest.main()
