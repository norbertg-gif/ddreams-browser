import builtins
from contextlib import closing, nullcontext
import io
import json
import os
import sqlite3
import tempfile
import threading
import time
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import MagicMock, patch

import numpy as np
from PIL import Image, ImageFilter

from ddreams_ai import AIService, SiglipEmbedder, VectorIndex, CLUSTER_VOCABULARY, COLOR_PALETTE, COLOR_VERSION, DIMENSION, FACE_VERSION, QUALITY_VERSION, RequestError, compute_quality, dominant_colors, learn_thresholds, palette_counts, text_probability_cutoff
from ddreams_ai import DECODER_VERSION, KEYWORD_VOCABULARY, RAW_EXTENSIONS, decode_photo
from faces_schema import ensure_faces_schema
from import_acdsee_faces import import_acdsee_faces


class FakePhoto:
    def __init__(self, name): self.name = name
    def __str__(self): return self.name
    def convert(self, mode): return Image.new(mode, (16, 16), "white")

def set_vectors(service, vectors):
    service._vectors = VectorIndex(list(vectors), np.stack(list(vectors.values())))

class ShutdownTests(unittest.TestCase):
    def test_stop_notifies_waiters_before_shutting_down_pools(self):
        service = AIService.__new__(AIService)
        service._condition = threading.Condition()
        service._stop = False
        service._loader = service._worker = service._face_worker = None
        service.faces_db = None
        service.db_lock = threading.Lock()
        service.db = MagicMock()
        waiting = threading.Event()
        woken = threading.Event()

        def wait_for_stop():
            with service._condition:
                waiting.set()
                if service._condition.wait_for(lambda: service._stop, timeout=5):
                    woken.set()

        def check_stop(*_args, **_kwargs):
            self.assertTrue(service._stop)
            self.assertTrue(woken.wait(1))

        service._query_pool = SimpleNamespace(shutdown=check_stop)
        service._vector_pool = SimpleNamespace(shutdown=check_stop)
        service._folder_catalog = SimpleNamespace(close=check_stop)
        waiter = threading.Thread(target=wait_for_stop, daemon=True)
        waiter.start()
        try:
            self.assertTrue(waiting.wait(1))
            service.stop()
        finally:
            with service._condition:
                service._stop = True
                service._condition.notify_all()
            waiter.join(1)


class SiglipLockTests(unittest.TestCase):
    def test_preprocessing_is_unlocked_but_model_and_tokenizer_are_serialized(self):
        embedder = SiglipEmbedder.__new__(SiglipEmbedder)
        embedder._lock = threading.RLock()
        calls = []
        tensor = MagicMock()
        tensor.to.return_value = tensor
        tensor.__truediv__.return_value = tensor

        def stage(name, locked):
            def call(*args):
                self.assertEqual(embedder._lock._is_owned(), locked, name)
                calls.append(name)
                return tensor
            return call

        embedder.torch = SimpleNamespace(stack=stage("stack", False), inference_mode=nullcontext,
                                         autocast=nullcontext, float16="fp16")
        embedder.preprocess = stage("preprocess", False)
        embedder.tokenizer = stage("tokenizer", True)
        embedder.model = SimpleNamespace(encode_image=stage("encode_image", True),
                                         encode_text=stage("encode_text", True))
        embedder.encode_images([object()])
        embedder._text_vector("demo")
        self.assertEqual(calls, ["preprocess", "stack", "encode_image", "tokenizer", "encode_text"])


class RawDecodeTests(unittest.TestCase):
    def test_all_catalogue_raw_extensions_are_recognized(self):
        formats_path = Path(__file__).resolve().parents[1] / "src-tauri" / "formats.json"
        formats = json.loads(formats_path.read_text(encoding="utf-8"))["formats"]
        self.assertEqual({entry["ext"] for entry in formats if entry["group"] == "raw"}, RAW_EXTENSIONS)

    def test_missing_unsupported_and_unusable_raw_previews_develop_at_half_size(self):
        import rawpy

        thumb_small = io.BytesIO()
        Image.new("RGB", (100, 80)).save(thumb_small, format="JPEG")
        cases = (
            rawpy.LibRawNoThumbnailError(),
            rawpy.LibRawUnsupportedThumbnailError(),
            SimpleNamespace(format=rawpy.ThumbFormat.JPEG, data=b"broken jpeg"),
            SimpleNamespace(format=rawpy.ThumbFormat.JPEG, data=thumb_small.getvalue()),
        )
        for case in cases:
            with self.subTest(preview=type(case).__name__):
                raw = MagicMock()
                raw.__enter__.return_value = raw
                if isinstance(case, Exception):
                    raw.extract_thumb.side_effect = case
                else:
                    raw.extract_thumb.return_value = case
                raw.postprocess.return_value = np.full((300, 600, 3), 140, dtype=np.uint8)
                with patch("rawpy.imread", return_value=raw) as read:
                    image = decode_photo({"path": "bez-náhľadu.DNG"}, max_side=256)
                read.assert_called_once_with("bez-náhľadu.DNG")
                raw.postprocess.assert_called_once_with(half_size=True, use_camera_wb=True, output_bps=8)
                raw.__exit__.assert_called_once()
                self.assertEqual((image.mode, image.size), ("RGB", (256, 128)))
                self.assertIn("ddreams_quality", image.info)

    def test_large_embedded_jpeg_avoids_developing_the_raw(self):
        import rawpy

        thumb = io.BytesIO()
        Image.new("RGB", (1000, 800)).save(thumb, format="JPEG")
        raw = MagicMock()
        raw.__enter__.return_value = raw
        raw.extract_thumb.return_value = SimpleNamespace(format=rawpy.ThumbFormat.JPEG, data=thumb.getvalue())
        with patch("rawpy.imread", return_value=raw):
            image = decode_photo({"path": "preview.cr3"})
        raw.postprocess.assert_not_called()
        raw.__exit__.assert_called_once()
        self.assertEqual(image.size, (512, 410))

    def test_ordinary_jpeg_with_raw_suffix_uses_pillow(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "obrázok.raw"
            Image.new("RGB", (64, 40), "red").save(path, format="JPEG")
            image = decode_photo({"path": str(path)})
        self.assertEqual((image.mode, image.size), ("RGB", (64, 40)))
        self.assertGreater(image.getpixel((0, 0))[0], 240)

    def test_heic_decodes_through_the_registered_opener(self):
        import pillow_heif

        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "fotka.heic"
            pillow_heif.from_pillow(Image.new("RGB", (64, 48), "red")).save(path)
            with decode_photo({"path": str(path)}) as image:
                self.assertEqual((image.mode, image.size), ("RGB", (64, 48)))
                self.assertGreater(image.getpixel((0, 0))[0], 235)
                self.assertIn("ddreams_quality", image.info)

    def test_too_big_is_propagated_and_closes_raw(self):
        import rawpy

        raw = MagicMock()
        raw.__enter__.return_value = raw
        raw.extract_thumb.side_effect = rawpy.LibRawNoThumbnailError()
        raw.postprocess.side_effect = rawpy.LibRawTooBigError()
        with patch("rawpy.imread", return_value=raw), self.assertRaises(rawpy.LibRawTooBigError):
            decode_photo({"path": "large.dng"})
        raw.__exit__.assert_called_once()


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


class FakeFaceDetector:
    def __init__(self, boxes=None): self.boxes = boxes if boxes is not None else [[30, 30, 100, 100]]
    def detect(self, image):
        boxes = np.asarray(self.boxes, dtype=np.float32).reshape(-1, 4)
        scores = np.ones(len(boxes), dtype=np.float32) * 0.99
        kps = np.tile(np.array([[40, 48], [80, 48], [60, 66], [45, 83], [75, 83]], dtype=np.float32), (len(boxes), 1, 1))
        return boxes, scores, kps


class FakeFaceEncoder:
    def __init__(self, vector=None):
        self.vector = np.asarray(vector if vector is not None else [1.0] + [0.0] * 511, dtype=np.float32)
    def embed(self, crops): return np.tile(self.vector, (len(crops), 1))


def _write_dbf(path, fields, records):
    descriptors = bytearray()
    for name, kind, length in fields:
        desc = bytearray(32); desc[:len(name)] = name.encode("ascii"); desc[11] = ord(kind); desc[16] = length
        descriptors.extend(desc)
    header_len, record_len = 32 + len(descriptors) + 1, 1 + sum(f[2] for f in fields)
    header = bytearray(32); header[0] = 3
    struct = __import__("struct")
    struct.pack_into("<I", header, 4, len(records)); struct.pack_into("<HH", header, 8, header_len, record_len)
    data = header + descriptors + b"\r"
    for deleted, values in records:
        raw = bytearray(b"*" if deleted else b" ")
        for (_, kind, length), value in zip(fields, values):
            if kind == "C": raw.extend(str(value).encode("utf-8")[:length].ljust(length, b" "))
            elif kind == "I": raw.extend(struct.pack("<i", int(value)))
            elif kind == "B": raw.extend(struct.pack("<d", float(value)))
        data.extend(raw)
    path.write_bytes(data + b"\x1a")


class FakeEmbedder:
    dimension = DIMENSION
    device = "cpu"

    def __init__(self, model_dir):
        pass

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

    def search_text(self, query, limit, index):
        ids, vectors, _ = index
        target = self.vector(query)
        scores = sorted(((aid, float(np.dot(vector, target))) for aid, vector in zip(ids, vectors)),
                        key=lambda pair: (-pair[1], pair[0]))
        return {"results": [{"asset_id": aid, "score": score, "prob": 0.9 if score > 0 else 0.01}
                            for aid, score in scores[:limit] if score >= 0.2], "searched": len(ids)}


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


class MapTests(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory();self.root=Path(self.temp.name)
        self.service=AIService(self.root/"map.sqlite","fake",embedder_factory=FakeEmbedder,decoder=lambda item:item["path"])
        self.service.start();self.assertTrue(self.service._model_ready.wait(5))
        vectors={}
        for cluster in range(8):
            for offset in range(4):
                v=np.zeros(DIMENSION,dtype=np.float32);v[cluster]=1;v[10+cluster*4+offset]=0.04
                vectors[cluster*10+offset+1]=v/np.linalg.norm(v)
        set_vectors(self.service, vectors)

    def tearDown(self):self.service.stop();self.temp.cleanup()

    def test_build_stores_every_point_and_is_deterministic(self):
        first=self.service._map_build();rows=self.service.db.execute("SELECT asset_id,x,y,cluster FROM map_points ORDER BY asset_id").fetchall()
        self.assertEqual(first["points"],32);self.assertEqual(len(rows),32)
        self.assertTrue(all(np.isfinite((x,y)).all() and 0<=x<=1 and 0<=y<=1 for _,x,y,_ in rows))
        assignments=[r[3] for r in rows]
        self.assertEqual(len(set(assignments[:4])),1);self.assertEqual(len(set(assignments[4:8])),1)
        snapshot=[tuple(r) for r in rows];self.service._map_build()
        self.assertEqual(snapshot,[tuple(r) for r in self.service.db.execute("SELECT asset_id,x,y,cluster FROM map_points ORDER BY asset_id")])

    def test_tsne_small_centroid_count(self):
        points=AIService._map_tsne(np.eye(3,dtype=np.float32))
        self.assertEqual(points.shape,(3,2));self.assertTrue(np.isfinite(points).all())

class LazyModelTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.calls = []
        def factory(path):
            self.calls.append(path)
            return FakeEmbedder(path)
        self.service = AIService(Path(self.temp.name) / "ai.sqlite", "fake", embedder_factory=factory, lazy_model=True)
        vectors = np.zeros((3, DIMENSION), dtype=np.float16)
        vectors[0, 0] = vectors[1, 0] = vectors[2, 1] = 1
        with self.service.db_lock:
            self.service.db.executemany("INSERT INTO embeddings VALUES(?,?,?,?,?,?)",
                [(i + 1, self.service.model_name, 1, 1, DIMENSION, vector.tobytes()) for i, vector in enumerate(vectors)])
            self.service.db.commit()
        self.service.start()

    def tearDown(self):
        self.service.stop()
        self.temp.cleanup()

    def test_keyword_matches_use_multiple_examples_and_exclude_rejections(self):
        set_vectors(self.service, {
            1: [1, 0, 0], 2: [0, 1, 0], 3: [1, 0, 0], 4: [0, 1, 0],
            5: [0, 0, 1], 6: [1, 0, 0],
        })
        self.assertEqual(self.service.execute({"cmd": "keyword_matches", "seeds": [1, 2, 99], "excluded": [6]}),
                         {"ids": [3, 4], "examples": 2})
        self.assertEqual(self.calls, [])

    def test_incoming_groups_avoid_similarity_chains_and_count_unindexed(self):
        set_vectors(self.service, {
            1: [1, 0], 2: [0.9, 0.4359], 3: [0.62, 0.7846], 4: [1, 0],
        })
        metadata = {1: (None, None), 2: (None, None), 3: (None, None),
                    4: ("Archive", None), 5: (None, None)}
        with patch.object(self.service, "_incoming_folder_metadata", return_value=metadata):
            result = self.service.execute({"cmd": "incoming_groups", "catalog": "catalog",
                                           "incoming": "Incoming", "extensions": ["jpg"]})
        self.assertEqual(result, {"groups": [[1, 2], [3]], "unindexed": 1, "omitted": 0})
        self.assertEqual(self.calls, [])

    def test_folder_examples_include_same_day_without_affecting_ranking(self):
        set_vectors(self.service, {
            1: [1, 0], 2: [1, 0], 3: [0.9, 0.4359], 4: [0.7, 0.7141], 5: [0, 1],
        })
        request = {"cmd": "suggest_folders", "asset_ids": [1],
                   "photos": [[1, None, "2024-07-23"], [2, "Archive", "2024-07-23"],
                              [3, "Archive", None], [4, "Archive", None], [5, "Other", None]]}
        self.assertEqual(self.service.execute(request), {"folders": [{"folder": "Archive", "share": 1.0}]})
        self.assertEqual(self.service.execute(dict(request, include_examples=True)),
                         {"folders": [{"folder": "Archive", "share": 1.0, "exampleIds": [2, 3]}]})

    def test_indexed_asset_ids_checks_stored_model_and_dimension_without_loading_vectors(self):
        self.assertIsNone(self.service._vectors)
        with self.service.db_lock, self.service.db:
            self.service.db.executemany("INSERT INTO embeddings VALUES(?,?,1,1,?,?)", [
                (11, self.service.model_name, DIMENSION, b""),
                (12, "other-model", DIMENSION, b""),
                (13, self.service.model_name, 2, b""),
            ])
        self.assertEqual(self.service.execute({"cmd": "indexed_asset_ids", "asset_ids": [11, 12, 13, 14]}),
                         {"asset_ids": [11]})
        self.assertIsNone(self.service._vectors)

    def test_lazy_start_does_not_import_torch(self):
        original_import = builtins.__import__
        def without_torch(name, *args, **kwargs):
            if name == "torch":
                self.fail("Torch was imported during lazy startup")
            return original_import(name, *args, **kwargs)

        with tempfile.TemporaryDirectory() as tmp, patch("builtins.__import__", side_effect=without_torch):
            service = AIService(Path(tmp) / "ai.sqlite", "fake", embedder_factory=self.fail, lazy_model=True)
            try:
                service.start()
                self.assertEqual(service.status()["model_state"], "unloaded")
                self.assertIsNone(service._cuda_torch)
            finally:
                service.stop()

    def test_cuda_duplicate_groups_equal_numpy_and_reuse_device_index(self):
        try:
            import torch
        except (ImportError, OSError, RuntimeError):
            self.skipTest("Torch unavailable")
        if not torch.cuda.is_available():
            self.skipTest("CUDA unavailable")
        rng = np.random.default_rng(115)
        values = rng.standard_normal((64, DIMENSION), dtype=np.float32)
        values /= np.linalg.norm(values, axis=1, keepdims=True)
        values[31] = values[1]
        values[40] = values[2] * 0.97 + values[3] * 0.24
        # This pair lands exactly between two half-precision scores.
        values[62:] = 0
        values[62, :2] = [1, 1 / 32]
        values[63, :2] = [0.9296875, 1 / 128]
        index = VectorIndex(list(range(1, 65)), values)
        self.service._vectors = index
        request = {"cmd": "duplicate_groups", "threshold": 0.93}
        with patch.object(self.service, "_cuda_torch", False):
            expected = self.service.execute(request)
        self.assertEqual(self.service.execute(request), expected)
        self.assertTrue(expected["groups"])
        _, first, _ = self.service._index_snapshot(use_cuda=True)
        self.assertTrue(first.is_cuda)
        self.assertEqual(first.data_ptr(), self.service._index_snapshot(use_cuda=True)[1].data_ptr())
        index.upsert([65], values[:1])
        _, grown, _ = self.service._index_snapshot(use_cuda=True)
        index.upsert([66], values[1:2])
        self.assertEqual(grown.data_ptr(), self.service._index_snapshot(use_cuda=True)[1].data_ptr())
        index.upsert([1], values[2:3])
        self.assertTrue(torch.equal(first[0].cpu(), torch.tensor(values[0], dtype=torch.float16)))
        index.remove({2, 65})
        ids, device, _ = self.service._index_snapshot(use_cuda=True)
        self.assertEqual(ids, index.snapshot()[0])
        np.testing.assert_array_equal(device.cpu().numpy(), index.snapshot()[1])
        with patch.object(self.service, "_cuda_torch", False):
            expected = self.service.execute(request)
        self.assertEqual(self.service.execute(request), expected)
        self.assertEqual(self.calls, [])

    def test_vectors_work_before_and_after_explicit_load(self):
        self.assertEqual(self.calls, [])
        self.assertEqual(self.service.status()["model_state"], "unloaded")
        before = self.service.execute({"cmd": "similar", "asset_id": 1})
        groups = self.service.execute({"cmd": "duplicate_groups", "threshold": 0.9})
        self.assertEqual([row["asset_id"] for row in before["results"]], [2])
        self.service.execute({"cmd": "load_model"})
        self.assertTrue(self.service._model_ready.wait(5))
        self.assertEqual(len(self.calls), 1)
        self.assertEqual(before, self.service.execute({"cmd": "similar", "asset_id": 1}))
        self.assertEqual(groups, self.service.execute({"cmd": "duplicate_groups", "threshold": 0.9}))

    def test_text_search_loads_model(self):
        self.assertEqual(self.calls, [])
        self.service.execute({"cmd": "search_text", "query": "east"})
        self.assertEqual(len(self.calls), 1)
        before = self.service._index_snapshot()[1]
        self.service.execute({"cmd": "search_text", "query": "west"})
        self.assertTrue(np.shares_memory(before, self.service._index_snapshot()[1]))

    def test_matrix_growth_updates_and_removal(self):
        index = VectorIndex([1, 2], np.array([[1, 0], [0, 1]], dtype=np.float16))
        old_ids, old_matrix, old_positions = index.snapshot()
        index.upsert([3], np.array([[1, 1]], dtype=np.float16))
        self.assertEqual(old_ids, [1, 2])
        self.assertEqual(old_positions, {1: 0, 2: 1})
        self.assertEqual(old_matrix.tolist(), [[1, 0], [0, 1]])
        capacity = index.capacity
        index.upsert([4], np.array([[0, 0]], dtype=np.float16))
        self.assertEqual(index.capacity, capacity)
        index.upsert([2], np.array([[2, 2]], dtype=np.float16))
        index.remove({1, 3})
        ids, matrix, positions = index.snapshot()
        self.assertEqual(ids, [2, 4])
        self.assertEqual(matrix.tolist(), [[2, 2], [0, 0]])
        self.assertEqual(positions, {2: 0, 4: 1})

    def test_snapshots_share_contiguous_matrix(self):
        first = self.service._index_snapshot()
        second = self.service._index_snapshot()
        self.assertEqual(first[1].dtype, np.float16)
        self.assertTrue(first[1].flags.c_contiguous)
        self.assertTrue(np.shares_memory(first[1], second[1]))

    def test_duplicates_match_float16_reference_at_threshold(self):
        rng = np.random.default_rng(123)
        values = rng.normal(size=(20, 32)).astype(np.float32)
        values /= np.linalg.norm(values, axis=1, keepdims=True)
        values[1] = values[0] * .93 + values[1] * .3675595
        self.service._vectors = VectorIndex(list(range(1, 21)), values)
        ids, matrix, _ = self.service._index_snapshot()
        scores = (matrix.astype(np.float32) @ matrix.astype(np.float32).T).astype(np.float16)
        for threshold in (.93, .985):
            neighbors = [[(j, float(scores[i, j])) for j in range(len(ids)) if i != j and scores[i, j] >= np.float16(threshold)] for i in range(len(ids))]
            order = sorted(range(len(ids)), key=lambda i: (-len(neighbors[i]), ids[i]))
            assigned, expected = set(), []
            for seed in order:
                if seed in assigned: continue
                members = [(other, score) for other, score in neighbors[seed] if other not in assigned]
                if not members: continue
                assigned.add(seed); assigned.update(other for other, _ in members)
                expected.append({"asset_ids": [ids[seed], *(ids[other] for other, _ in members)],
                                 "min_sim": min(score for _, score in members), "max_sim": max(score for _, score in members)})
            expected.sort(key=lambda group: (-len(group["asset_ids"]), -group["max_sim"], group["asset_ids"][0]))
            self.assertEqual(self.service.execute({"cmd": "duplicate_groups", "threshold": threshold})["groups"], expected)

    def test_similarity_probability_reads_only_checkpoint_scalars(self):
        from safetensors.numpy import save_file
        path = Path(self.temp.name) / "open_clip_model.fp16.safetensors"
        save_file({"logit_scale": np.array([0.0], dtype=np.float16), "logit_bias": np.array([0.0], dtype=np.float16)}, str(path))
        self.service.model_dir = self.temp.name
        result = self.service.execute({"cmd": "similar", "asset_id": 1})
        self.assertAlmostEqual(result["results"][0]["prob"], 1 / (1 + np.exp(-1)), places=6)
        self.assertEqual(self.calls, [])

    def test_other_vector_commands_use_only_stored_data(self):
        self.service.execute({"cmd": "set_labels", "kind": "category", "pairs": [[1, 7]], "parents": {"7": None}})
        self.assertIn("2", self.service.execute({"cmd": "suggest", "kind": "category", "asset_ids": [2]})["results"])
        self.service.execute({"cmd": "suggest_batch", "kind": "category", "asset_ids": "unlabeled"})
        self.assertEqual(self.service.execute({"cmd": "order_by_similarity", "asset_ids": [1, 2, 3]})["asset_ids"][0], 1)
        self.assertEqual(self.service.execute({"cmd": "series", "items": [{"asset_id": 1, "t": 1}, {"asset_id": 2, "t": 2}], "max_gap": 10, "min_sim": .9})["groups"][0]["ids"], [1, 2])
        self.assertEqual(self.service.execute({"cmd": "map_build"})["points"], 3)
        self.service.execute({"cmd": "clusters", "min_size": 2, "min_sim": .7})
        self.assertEqual(self.calls, [])

    def test_queued_items_wait_without_loading_and_stop_cleanly(self):
        path = Path(self.temp.name) / "queued.jpg"
        path.write_bytes(b"photo")
        stat = path.stat()
        self.service.execute({"cmd": "index", "append": True, "items": [
            {"asset_id": 9, "path": str(path), "size": stat.st_size, "mtime_ns": stat.st_mtime_ns}]})
        self.assertEqual(self.calls, [])
        self.assertGreaterEqual(self.service.status()["waiting"], 1)
        self.assertEqual(self.service.status()["model_state"], "unloaded")

    def test_stored_text_names_do_not_load_model(self):
        text = "a photo of krajina"
        vector = np.zeros(DIMENSION, dtype=np.float16)
        vector[0] = 1
        with self.service.db_lock:
            self.service.db.execute("INSERT INTO text_vectors VALUES(?,?,?)", (self.service.model_name, text, vector.tobytes()))
            self.service.db.commit()
        self.assertIsNone(self.service._cluster_names())
        with self.service.db_lock:
            self.service.db.executemany("INSERT OR IGNORE INTO text_vectors VALUES(?,?,?)",
                [(self.service.model_name, f"a photo of {word}", vector.tobytes()) for word in CLUSTER_VOCABULARY])
            self.service.db.commit()
        self.assertIn("krajina", self.service._cluster_names())
        self.assertEqual(self.calls, [])

    def test_content_keywords_can_find_a_new_word_from_cached_vectors(self):
        photo = np.zeros(DIMENSION, dtype=np.float16)
        photo[0] = 1
        unrelated = np.zeros_like(photo)
        unrelated[1] = 1
        self.service._vectors = VectorIndex([1, 2], np.stack([photo, unrelated]))
        with self.service.db_lock:
            self.service.db.executemany("INSERT INTO text_vectors VALUES(?,?,?)", [
                (self.service.model_name, f"a photo of {word}",
                 (photo if word == "slon" else np.zeros_like(photo)).tobytes())
                for word in KEYWORD_VOCABULARY
            ])
            self.service.db.commit()
        self.assertEqual(self.service.execute({"cmd": "content_keywords", "asset_id": 1}), {"words": ["slon"]})
        self.assertEqual(self.service.execute({"cmd": "content_keywords", "asset_id": 2}), {"words": []})
        self.assertEqual(self.calls, [])

    def test_model_computed_names_are_persisted(self):
        self.service.execute({"cmd": "load_model"})
        self.assertTrue(self.service._model_ready.wait(5))
        self.service._embedder._text_vector = lambda text: np.ones(DIMENSION, dtype=np.float32)
        self.assertIn("krajina", self.service._cluster_names())
        row = self.service.db.execute("SELECT vector FROM text_vectors WHERE model=? AND text=?",
                                      (self.service.model_name, "a photo of krajina")).fetchone()
        self.assertEqual(len(row[0]), DIMENSION * 2)
        self.service._embedder = None
        self.assertIn("krajina", self.service._cluster_names())


class RawDecoderRetryTests(unittest.TestCase):
    def test_upgrade_from_raw_decoder_only_retries_new_image_formats(self):
        with tempfile.TemporaryDirectory() as tmp, patch.dict(os.environ, {"DDREAMS_CATALOG": ""}):
            root = Path(tmp)
            db = root / "ai.sqlite"
            with sqlite3.connect(root / "catalog.sqlite") as catalog:
                catalog.execute("CREATE TABLE assets(id INTEGER PRIMARY KEY, path TEXT)")
                catalog.executemany("INSERT INTO assets VALUES(?,?)", [
                    (i, f"photo.{ext}") for i, ext in enumerate(
                        ("HEIC", "heif", "avif", "jp2", "j2k", "hif", "jpc", "dng", "jpg"), 1)
                ])
            catalog.close()
            svc = AIService(db, "fake")
            try:
                with svc.db:
                    svc.db.execute("UPDATE meta SET value='1' WHERE key='raw_decoder_version'")
                    svc.db.executemany("INSERT INTO failures VALUES(?,10,20,'UnidentifiedImageError: decode',0)",
                                       [(i,) for i in range(1, 10)])
                with svc.faces_db:
                    svc.faces_db.executemany("INSERT INTO face_scan VALUES(?,10,20,?,0)",
                                             [(i, FACE_VERSION) for i in range(1, 10)])
            finally:
                svc.stop()
            svc = AIService(db, "fake")
            try:
                for conn, table in [(svc.db, "failures"), (svc.faces_db, "face_scan")]:
                    self.assertEqual(conn.execute(f"SELECT asset_id FROM {table} ORDER BY asset_id").fetchall(),
                                     [(8,), (9,)])
            finally:
                svc.stop()

    def test_startup_retries_only_targeted_assets_once_and_preserves_faces(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            db = root / "ai.sqlite"
            catalog_path = root / "custom.sqlite"
            catalog = sqlite3.connect(catalog_path)
            with catalog:
                catalog.execute("CREATE TABLE assets(id INTEGER PRIMARY KEY, path TEXT)")
                catalog.executemany("INSERT INTO assets VALUES(?,?)", [
                    (1, "first.dng"), (2, "misnamed.raw"), (3, "thumb.dng"), (4, "camera.RAF"),
                    (5, "other.jpg"), (6, "protected.dng"), (7, "large.dng"), (8, "unchanged.dng"),
                ])
            catalog.close()
            with patch.dict(os.environ, {"DDREAMS_CATALOG": str(catalog_path)}):
                service = AIService(db, "fake", embedder_factory=FakeEmbedder)
                try:
                    with service.db:
                        service.db.execute("UPDATE meta SET value='0' WHERE key='raw_decoder_version'")
                        service.db.executemany("INSERT INTO failures VALUES(?,10,20,?,0)", [
                            (1, "LibRawNoThumbnailError: No thumbnail in file"),
                            (2, "UnidentifiedImageError: invalid preview"),
                            (3, "LibRawUnsupportedThumbnailError: unsupported thumbnail"),
                            (4, "LibRawDataError: corrupted data"),
                            (5, "ValueError: unrelated"),
                            (6, "LibRawNoThumbnailError: No thumbnail in file"),
                            (7, "LibRawTooBigError: too big"),
                            (9, "LibRawNoThumbnailError: no path in catalogue"),
                        ])
                    with service.faces_db:
                        service.faces_db.executemany("INSERT INTO face_scan VALUES(?,10,20,?,0)",
                                                     [(i, FACE_VERSION) for i in range(1, 10)])
                        service.faces_db.execute("INSERT INTO persons(id,name) VALUES(11,'Ľudmila')")
                        service.faces_db.execute("""INSERT INTO faces(id,asset_id,source,person_id,state)
                                                   VALUES(16,6,'ai',11,'confirmed')""")
                        service.faces_db.execute("INSERT INTO face_thumbs VALUES(16,X'0102')")
                        service.faces_db.execute("INSERT INTO face_rejected VALUES(16,11)")
                finally:
                    service.stop()

                reopened = AIService(db, "fake", embedder_factory=FakeEmbedder)
                try:
                    self.assertEqual(reopened.db.execute("SELECT asset_id FROM failures ORDER BY asset_id").fetchall(), [(5,), (7,)])
                    self.assertEqual(reopened.faces_db.execute("SELECT asset_id FROM face_scan ORDER BY asset_id").fetchall(),
                                     [(5,), (6,), (7,), (8,)])
                    self.assertEqual(reopened.faces_db.execute("SELECT id,asset_id,person_id,state FROM faces").fetchall(),
                                     [(16, 6, 11, "confirmed")])
                    self.assertEqual(reopened.faces_db.execute("SELECT id,name FROM persons").fetchall(), [(11, "Ľudmila")])
                    self.assertEqual(reopened.faces_db.execute("SELECT * FROM face_thumbs").fetchall(), [(16, b"\x01\x02")])
                    self.assertEqual(reopened.faces_db.execute("SELECT * FROM face_rejected").fetchall(), [(16, 11)])
                    self.assertEqual(reopened.db.execute("SELECT value FROM meta WHERE key='raw_decoder_version'").fetchone(),
                                     (str(DECODER_VERSION),))
                    items = [{"asset_id": i, "path": str(root / f"{i}.dng"), "size": 10, "mtime_ns": 20}
                             for i in (1, 6, 7)]
                    result = reopened.execute({"cmd": "index", "items": items, "append": True})
                    self.assertEqual((result["queued"], result["failed_before"]), (2, 1))
                    self.assertEqual([item["asset_id"] for item in reopened._face_scan_queue], [1])
                    with reopened.db:
                        reopened.db.execute("INSERT INTO failures VALUES(1,10,20,'LibRawNoThumbnailError: again',0)")
                    with reopened.faces_db:
                        reopened.faces_db.execute("INSERT INTO face_scan VALUES(1,10,20,?,0)", (FACE_VERSION,))
                finally:
                    reopened.stop()

                repeated = AIService(db, "fake", embedder_factory=FakeEmbedder)
                try:
                    self.assertEqual(repeated.db.execute("SELECT asset_id FROM failures ORDER BY asset_id").fetchall(),
                                     [(1,), (5,), (7,)])
                    self.assertEqual(repeated.faces_db.execute("SELECT asset_id FROM face_scan ORDER BY asset_id").fetchall(),
                                     [(1,), (5,), (6,), (7,), (8,)])
                finally:
                    repeated.stop()


class FaceTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(); self.root = Path(self.temp.name)
        self.service = AIService(self.root / "ai.sqlite", "fake", embedder_factory=FakeEmbedder,
                                 decoder=lambda item: Image.new("RGB", (160, 120), "white"),
                                 face_detector=FakeFaceDetector(), face_encoder=FakeFaceEncoder())

    def tearDown(self):
        self.service.stop(); self.temp.cleanup()

    def _person(self, name="Jana"):
        return self.service.execute({"cmd": "faces_person_create", "name": name})["id"]

    def test_failed_face_scan_is_remembered_and_changed_file_is_requeued(self):
        path = self.root / "broken.jpg"
        path.write_bytes(b"broken")
        stat = path.stat()
        item = {"asset_id": 17, "path": str(path), "size": stat.st_size, "mtime_ns": stat.st_mtime_ns}
        def fail(_item):
            raise ValueError("bad image")
        self.service.decoder = fail
        self.service.start()
        self.assertTrue(self.service._model_ready.wait(5))
        self.service.execute({"cmd": "index", "items": [item], "append": True})
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            with self.service.faces_lock:
                row = self.service.faces_db.execute("SELECT size,mtime_ns,version FROM face_scan WHERE asset_id=17").fetchone()
            if row:
                break
            time.sleep(0.02)
        self.assertEqual(row, (stat.st_size, stat.st_mtime_ns, FACE_VERSION))
        with self.service.db_lock:
            self.service.db.execute("INSERT OR REPLACE INTO failures(asset_id,size,mtime_ns,error,at) VALUES(?,?,?,?,?)",
                                    (item["asset_id"], item["size"], item["mtime_ns"], "test", time.time()))
            self.service.db.execute("INSERT OR REPLACE INTO colors VALUES(?,?,?,?,?,?)", (17, item["size"], item["mtime_ns"], "{}", "[]", COLOR_VERSION))
            self.service.db.execute("INSERT OR REPLACE INTO quality VALUES(?,?,?,?,?,?,?,?)", (17, item["size"], item["mtime_ns"], 0, 0, 0, 0, QUALITY_VERSION))
            self.service.db.commit()
        self.assertNotIn(17, self.service.execute({"cmd": "light_sync", "asset_ids": [17]})["asset_ids"])
        self.service.execute({"cmd": "index", "items": [item], "append": True})
        with self.service._condition:
            self.assertFalse(any(entry["asset_id"] == 17 for entry in self.service._face_scan_queue))
        path.write_bytes(b"changed!")
        changed = path.stat()
        with self.service._condition:
            self.service._paused = True
        self.service.execute({"cmd": "index", "items": [{**item, "size": changed.st_size, "mtime_ns": changed.st_mtime_ns}], "append": True})
        with self.service._condition:
            self.assertTrue(any(entry["asset_id"] == 17 for entry in self.service._face_scan_queue))

    def test_light_sync_finds_missing_prunes_and_rejects_empty(self):
        vector = np.zeros(DIMENSION, dtype=np.float16).tobytes()
        with self.service.db_lock:
            for aid in [1, 2, 3, 4]:
                self.service.db.execute("INSERT INTO embeddings VALUES(?,?,?,?,?,?)", (aid, self.service.model_name, 1, 2, DIMENSION, vector))
                self.service.db.execute("INSERT INTO colors VALUES(?,?,?,?,?,?)", (aid, 1, 2, "{}", "[]", COLOR_VERSION))
                self.service.db.execute("INSERT INTO quality VALUES(?,?,?,?,?,?,?,?)", (aid, 1, 2, 0, 0, 0, 0, QUALITY_VERSION))
            self.service.db.execute("DELETE FROM quality WHERE asset_id=2")
            self.service.db.commit()
        with self.service.faces_lock:
            self.service.faces_db.executemany("INSERT INTO face_scan VALUES(?,?,?,?,?)", [(aid, 1, 2, FACE_VERSION, 0) for aid in [1, 2, 3, 4]])
            face_id = self.service.faces_db.execute("INSERT INTO faces(asset_id,source,state) VALUES(4,'ai','confirmed')").lastrowid
            self.service.faces_db.execute("INSERT INTO face_thumbs(face_id,jpeg) VALUES(?,?)", (face_id, b"jpeg"))
            self.service.faces_db.execute("INSERT INTO face_rejected(face_id,person_id) VALUES(?,?)", (face_id, 7))
            self.service.faces_db.commit()
        result = self.service.execute({"cmd": "light_sync", "asset_ids": [1, 2, 3, 5]})
        self.assertEqual(result, {"asset_ids": [2, 5], "pruned": 1})
        self.assertIsNone(self.service.db.execute("SELECT 1 FROM embeddings WHERE asset_id=4").fetchone())
        self.assertIsNotNone(self.service.faces_db.execute("SELECT 1 FROM face_scan WHERE asset_id=4").fetchone())
        self.assertIsNotNone(self.service.faces_db.execute("SELECT 1 FROM faces WHERE asset_id=4").fetchone())
        self.assertIsNotNone(self.service.faces_db.execute("SELECT 1 FROM face_thumbs WHERE face_id=?", (face_id,)).fetchone())
        self.assertIsNotNone(self.service.faces_db.execute("SELECT 1 FROM face_rejected WHERE face_id=?", (face_id,)).fetchone())
        with self.assertRaises(RequestError):
            self.service.execute({"cmd": "light_sync", "asset_ids": []})
        self.assertIsNotNone(self.service.db.execute("SELECT 1 FROM embeddings WHERE asset_id=1").fetchone())

    def test_asset_assign_uses_single_eligible_face_or_inserts_photo_row(self):
        person = self._person()
        with self.service.faces_lock:
            eligible = self.service.faces_db.execute("INSERT INTO faces(asset_id,x,y,w,h,source,state) VALUES(1,0,0,1,1,'ai','unknown')").lastrowid
            self.service.faces_db.execute("INSERT INTO faces(asset_id,x,y,w,h,source,state) VALUES(2,0,0,1,1,'ai','unknown')")
            self.service.faces_db.execute("INSERT INTO faces(asset_id,x,y,w,h,source,state) VALUES(2,0,0,1,1,'ai','auto')")
            self.service.faces_db.commit()
        result = self.service.execute({"cmd":"faces_asset_assign","asset_ids":[1,2],"person_id":person})
        self.assertEqual(result["changed"], 2)
        row = self.service.faces_db.execute("SELECT person_id,state FROM faces WHERE id=?", (eligible,)).fetchone()
        self.assertEqual(row, (person, "confirmed"))
        photo = self.service.faces_db.execute("SELECT x,y,w,h,person_id,state FROM faces WHERE asset_id=2 AND person_id=?",(person,)).fetchone()
        self.assertEqual(photo, (None,None,None,None,person,"confirmed"))

    def test_asset_assign_confirms_existing_nonignored_person_and_remove_branches(self):
        person = self._person()
        with self.service.faces_lock:
            boxed = self.service.faces_db.execute("INSERT INTO faces(asset_id,x,y,w,h,source,person_id,state) VALUES(3,0,0,1,1,'ai',?,'auto')", (person,)).lastrowid
            photo = self.service.faces_db.execute("INSERT INTO faces(asset_id,x,y,w,h,source,person_id,state) VALUES(4,NULL,NULL,NULL,NULL,'ai',?,'confirmed')", (person,)).lastrowid
            removable = self.service.faces_db.execute("INSERT INTO faces(asset_id,x,y,w,h,source,person_id,state) VALUES(5,0,0,1,1,'ai',?,'confirmed')", (person,)).lastrowid
            self.service.faces_db.commit()
        self.assertEqual(self.service.execute({"cmd":"faces_asset_assign","asset_ids":[3],"person_id":person})["changed"],1)
        result=self.service.execute({"cmd":"faces_asset_remove","asset_ids":[4,5],"person_id":person})
        self.assertEqual(result["changed"],2)
        self.assertIsNone(self.service.faces_db.execute("SELECT 1 FROM faces WHERE id=?",(photo,)).fetchone())
        self.assertEqual(self.service.faces_db.execute("SELECT person_id,state FROM faces WHERE id=?",(removable,)).fetchone(),(None,"unknown"))
        self.assertEqual(self.service.faces_db.execute("SELECT count(*) FROM face_rejected WHERE face_id=? AND person_id=?",(removable,person)).fetchone()[0],1)
        self.assertEqual(self.service.execute({"cmd":"faces_for_assets","asset_ids":[3,4,5]})["faces"][0]["face_id"], boxed)

    def test_boxless_confirmed_face_survives_scan(self):
        person=self._person()
        with self.service.faces_lock:
            self.service.faces_db.execute("INSERT INTO faces(asset_id,x,y,w,h,source,person_id,state) VALUES(9,NULL,NULL,NULL,NULL,'ai',?,'confirmed')",(person,))
            self.service.faces_db.commit()
        self.service._scan_face_asset({"asset_id":9,"size":1,"mtime_ns":2},Image.new("RGB",(160,120),"white"))
        self.assertEqual(self.service.faces_db.execute("SELECT count(*) FROM faces WHERE asset_id=9 AND person_id=? AND state='confirmed'",(person,)).fetchone()[0],1)

    def test_scan_matches_imported_box_keeps_name_and_stores_vector(self):
        person = self._person()
        with self.service.faces_lock:
            cur = self.service.faces_db.execute("INSERT INTO faces(asset_id,x,y,w,h,source,ext_id,person_id,state) VALUES(7,.18,.24,.44,.58,'acdsee',77,?,'confirmed')", (person,))
            fid = cur.lastrowid; self.service.faces_db.commit()
        count = self.service._scan_face_asset({"asset_id": 7, "size": 1, "mtime_ns": 2}, Image.new("RGB", (160, 120), "white"))
        row = self.service.faces_db.execute("SELECT x,y,w,h,person_id,state,vector FROM faces WHERE id=?", (fid,)).fetchone()
        self.assertEqual(count, 1); self.assertEqual((row[4], row[5]), (person, "confirmed"))
        self.assertEqual((row[0], row[1], row[2], row[3]), (.18, .24, .44, .58))
        self.assertEqual(len(row[6]), 1024); self.assertEqual(self.service.faces_db.execute("SELECT count(*) FROM face_thumbs WHERE face_id=?", (fid,)).fetchone()[0], 1)

    def test_unmatched_is_unknown_and_small_detection_is_dropped(self):
        self.service._face_detector = FakeFaceDetector([[30, 30, 100, 100], [4, 4, 20, 20]])
        self.service._scan_face_asset({"asset_id": 8, "size": 1, "mtime_ns": 2}, Image.new("RGB", (160, 120), "white"))
        rows = self.service.faces_db.execute("SELECT state,person_id FROM faces WHERE asset_id=8").fetchall()
        self.assertEqual(rows, [("unknown", None)])

    def test_recognition_accept_reject_threshold_and_rejected_person(self):
        p1, p2 = self._person("A"), self._person("B")
        with self.service.faces_lock:
            for vector, person in (([1, 0], p1), ([1, 0], p1), ([0.8, 0.6], p2)):
                v = np.zeros(512, dtype=np.float16); v[:2] = vector
                self.service.faces_db.execute("INSERT INTO faces(asset_id,x,y,w,h,source,person_id,state,vector) VALUES(1,0,0,1,1,'acdsee',?,'confirmed',?)", (person, v.tobytes()))
            cur = self.service.faces_db.execute("INSERT INTO faces(asset_id,x,y,w,h,source,state,vector) VALUES(2,0,0,1,1,'ai','unknown',?)", (np.eye(1, 512, dtype=np.float16).tobytes(),))
            fid = cur.lastrowid; self.service.faces_db.commit(); self.service._refresh_face_references_locked()
        self.service._recognize_face_ids([fid])
        self.assertEqual(self.service.faces_db.execute("SELECT person_id,state FROM faces WHERE id=?", (fid,)).fetchone(), (p1, "auto"))
        self.service.execute({"cmd": "faces_set", "face_ids": [fid], "action": "reject"})
        row = self.service.faces_db.execute("SELECT person_id,state FROM faces WHERE id=?", (fid,)).fetchone()
        self.assertEqual(row, (p2, "auto"))
        self.assertEqual(self.service.faces_db.execute("SELECT count(*) FROM face_rejected WHERE face_id=? AND person_id=?", (fid, p1)).fetchone()[0], 1)

    def test_suggested_is_cleared_when_no_reference_matches(self):
        person = self._person()
        with self.service.faces_lock:
            vec = np.zeros(512, dtype=np.float16); vec[1] = 1
            self.service.faces_db.execute("INSERT INTO faces(asset_id,x,y,w,h,source,person_id,state,vector) VALUES(1,0,0,1,1,'acdsee',?,'confirmed',?)", (person, vec.tobytes()))
            fid = self.service.faces_db.execute("INSERT INTO faces(asset_id,x,y,w,h,source,person_id,state,vector) VALUES(2,0,0,1,1,'acdsee',?,'suggested',?)", (person, np.eye(1,512,dtype=np.float16).tobytes())).lastrowid
            self.service.faces_db.commit(); self.service._refresh_face_references_locked()
        self.service._recognize_face_ids([fid])
        self.assertEqual(self.service.faces_db.execute("SELECT person_id,state FROM faces WHERE id=?", (fid,)).fetchone(), (None, "unknown"))

    def test_faces_set_actions_and_person_counts(self):
        person = self._person(); vec = np.eye(1, 512, dtype=np.float16).tobytes()
        with self.service.faces_lock:
            f1 = self.service.faces_db.execute("INSERT INTO faces(asset_id,x,y,w,h,source,person_id,state,vector) VALUES(1,0,0,1,1,'ai',?,'auto',?)", (person, vec)).lastrowid
            f2 = self.service.faces_db.execute("INSERT INTO faces(asset_id,x,y,w,h,source,state,vector) VALUES(2,0,0,1,1,'ai','unknown',?)", (vec,)).lastrowid
            self.service.faces_db.commit()
        self.assertEqual(self.service.execute({"cmd": "faces_set", "face_ids": [f1], "action": "confirm"})["changed"], 1)
        self.assertEqual(self.service.execute({"cmd": "faces_set", "face_ids": [f2], "action": "assign", "person_id": person})["changed"], 1)
        self.assertEqual(self.service.execute({"cmd": "faces_set", "face_ids": [f2], "action": "clear"})["changed"], 1)
        self.service.execute({"cmd": "faces_set", "face_ids": [f1], "action": "ignore"})
        counts = self.service.execute({"cmd": "faces_persons"})["persons"][0]
        self.assertEqual((counts["confirmed"], counts["auto"], counts["assets"]), (0, 0, 0))

    def test_acdsee_dbf_import_y_flip_utf8_deleted_records_and_idempotence(self):
        from struct import pack
        acdsee = self.root / "acdsee"; acdsee.mkdir()
        _write_dbf(acdsee / "Person.dbf", [("PERSON_ID", "I", 4), ("NAME", "C", 40)], [(False, [4, "Žofia Čierna"]), (True, [5, "Deleted"])])
        _write_dbf(acdsee / "Face.dbf", [("ASSET_ID", "I", 4), ("DBB_X", "B", 8), ("DBB_Y", "B", 8), ("DBB_W", "B", 8), ("DBB_H", "B", 8), ("DESC_ID", "I", 4), ("FACE_ID", "I", 4)], [(False, [12, .1, .2, .3, .4, 90, 100]), (True, [12, .1, .2, .3, .4, 91, 101])])
        _write_dbf(acdsee / "RecognizedPerson.dbf", [("PERSON_ID", "I", 4), ("DESC_ID_R", "I", 4), ("ISAUTO", "I", 4)], [(False, [4, 90, -1])])
        catalog = self.root / "catalog.sqlite"; conn = sqlite3.connect(catalog); conn.execute("CREATE TABLE assets(id INTEGER PRIMARY KEY,is_listed INTEGER)"); conn.executemany("INSERT INTO assets VALUES(?,?)", [(12, 1), (13, 0)]); conn.commit(); conn.close()
        target = self.root / "faces-import.sqlite"
        first = import_acdsee_faces(acdsee, catalog, target); second = import_acdsee_faces(acdsee, catalog, target)
        self.assertEqual(first["faces"], {"confirmed": 1}); self.assertEqual(second["faces"], {}); self.assertEqual(second["skipped"], 1)
        db = sqlite3.connect(target); row = db.execute("SELECT asset_id,x,y,w,h,state FROM faces").fetchone(); name = db.execute("SELECT name FROM persons").fetchone()[0]; db.close()
        self.assertEqual(row, (12, .1, .4, .3, .4, "confirmed")); self.assertEqual(name, "Žofia Čierna")

    def _insert_cluster_face(self, asset_id, vector, state="unknown", person_id=None, det_score=.9):
        vector = np.asarray(vector, dtype=np.float32)
        vector /= np.linalg.norm(vector)
        with self.service.faces_lock:
            cursor = self.service.faces_db.execute("INSERT INTO faces(asset_id,x,y,w,h,det_score,source,person_id,state,vector) VALUES(?,0,0,1,1,?,'ai',?,?,?)", (asset_id, det_score, person_id, state, vector.astype(np.float16).tobytes()))
            if state in ("confirmed", "acdsee") and person_id is not None: self.service._refresh_face_references_locked()
            self.service.faces_db.commit()
        return int(cursor.lastrowid)

    def test_faces_clusters_find_two_identities_ignore_noise_and_are_deterministic(self):
        for identity, axis in ((0, 0), (1, 1)):
            for offset in range(5):
                vector = np.zeros(512, dtype=np.float32); vector[axis] = 1; vector[10 + offset] = .04
                self._insert_cluster_face(identity * 10 + offset, vector)
        for offset in range(3):
            vector = np.zeros(512, dtype=np.float32); vector[30 + offset] = 1
            self._insert_cluster_face(30 + offset, vector)
        first = self.service.execute({"cmd": "faces_clusters", "min_size": 4})
        second = self.service.execute({"cmd": "faces_clusters", "min_size": 4})
        self.assertTrue(first["available"])
        self.assertEqual(first, second)
        self.assertEqual([group["total"] for group in first["groups"]], [5, 5])
        self.assertEqual(first["unknown"], 13)

    def test_faces_clusters_min_size_and_named_ignored_exclusion(self):
        for i in range(4):
            vector = np.zeros(512, dtype=np.float32); vector[0] = 1; vector[20 + i] = .03
            self._insert_cluster_face(i + 1, vector)
        person = self._person()
        self._insert_cluster_face(50, np.eye(1, 512, dtype=np.float32)[0], "confirmed", person)
        self._insert_cluster_face(51, np.eye(1, 512, dtype=np.float32)[0], "ignored")
        result = self.service.execute({"cmd": "faces_clusters", "min_size": 4})
        self.assertEqual(result["unknown"], 4)
        self.assertEqual(len(result["groups"]), 1)
        self.assertEqual(self.service.execute({"cmd": "faces_clusters", "min_size": 5})["groups"], [])

    def test_faces_clusters_guess_uses_nearest_reference_face(self):
        person = self._person("Jana")
        reference = np.zeros(512, dtype=np.float32); reference[0] = 1
        self._insert_cluster_face(90, reference, "confirmed", person)
        for i in range(4):
            vector = reference.copy(); vector[1 + i] = .2
            self._insert_cluster_face(100 + i, vector)
        group = self.service.execute({"cmd": "faces_clusters"})["groups"][0]
        self.assertEqual(group["guess"]["person_id"], person)
        self.assertEqual(group["guess"]["name"], "Jana")

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

    def test_prune_chunks_ids_and_includes_colors_only(self):
        with self.service.db_lock:
            self.service.db.executemany("INSERT INTO failures VALUES(?,?,?,?,?)",
                                       [(aid, 1, 2, "synthetic", 0.0) for aid in range(1, 700)])
            self.service.db.executemany("INSERT INTO colors VALUES(?,?,?,?,?,?)",
                                       [(aid, 1, 2, "{}", "[]", COLOR_VERSION) for aid in range(700, 1101)])
            self.service.db.commit()
            previous_limit = self.service.db.setlimit(sqlite3.SQLITE_LIMIT_VARIABLE_NUMBER, 600)
        try:
            self.assertEqual(self.index([], prune=True)["pruned"], 1100)
            with self.service.db_lock:
                self.assertEqual(self.service.db.execute("SELECT count(*) FROM colors").fetchone()[0], 0)
                self.assertEqual(self.service.db.execute("SELECT count(*) FROM failures").fetchone()[0], 0)
        finally:
            with self.service.db_lock:
                self.service.db.setlimit(sqlite3.SQLITE_LIMIT_VARIABLE_NUMBER, previous_limit)

    def test_cuda_upload_does_not_hold_db_lock_and_retries_after_mutation(self):
        started, released = threading.Event(), threading.Event()
        uploads, output = [], []

        class DeviceArray(np.ndarray):
            def copy_(self, values):
                self[...] = values

        def empty(shape, **kwargs):
            uploads.append(shape)
            if len(uploads) == 1:
                started.set()
                if not released.wait(5):
                    raise RuntimeError("upload was not released")
            return np.empty(shape, dtype=np.float16).view(DeviceArray)

        with self.service.db_lock:
            set_vectors(self.service, {1: np.ones(4, dtype=np.float16)})
            self.service._cuda_torch = SimpleNamespace(empty=empty, from_numpy=lambda array: array, float16=np.float16)
        worker = threading.Thread(target=lambda: output.append(self.service._index_snapshot(use_cuda=True)))
        worker.start()
        try:
            self.assertTrue(started.wait(5))
            acquired = self.service.db_lock.acquire(timeout=1)
            self.assertTrue(acquired)
            try:
                self.service._vectors.upsert([2], np.full((1, 4), 2, dtype=np.float16))
            finally:
                self.service.db_lock.release()
        finally:
            released.set()
            worker.join(5)
        self.assertFalse(worker.is_alive())
        self.assertEqual(len(uploads), 2)
        self.assertEqual(output[0][0], [1, 2])
        np.testing.assert_array_equal(output[0][1], [[1]*4, [2]*4])

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
        set_vectors(self.service, vectors)
        for asset_id, value in (sharp or {}).items():
            self.service.db.execute("INSERT OR REPLACE INTO quality(asset_id,sharp) VALUES(?,?)", (asset_id, value))
        self.service.db.commit()
        return self.service.execute({"cmd": "series", "items": [{"asset_id": aid, "t": stamp} for aid, stamp in timed_ids],
                                     "max_gap": max_gap, "min_sim": min_sim})["groups"]

    def test_indexed_assets_event_waits_for_face_scans_and_only_emits_once(self):
        self.service._newly_indexed_ids.update([8, 7])
        self.service._face_active_ids.add(7)
        self.service._emit_indexed_assets()
        self.assertFalse(any(event.get("event") == "indexed_assets" for event in self.events))
        self.service._face_active_ids.clear()
        self.service._emit_indexed_assets()
        self.service._emit_indexed_assets()
        self.assertEqual([event for event in self.events if event.get("event") == "indexed_assets"],
                         [{"event": "indexed_assets", "asset_ids": [7, 8]}])

    def test_folder_suggestions_aggregate_votes_and_exclude_the_shoot_and_incoming(self):
        scores = {
            1: (1, 0), 2: (0, 1), 3: (1, 0), 4: (1, 0),
            5: (0.75, 0), 6: (0, 0.75), 7: (0.85, 0), 8: (0, 0.45), 9: (-1, 0),
        }
        set_vectors(self.service, {i: np.asarray(parts, dtype=np.float32) for i, parts in scores.items()})
        photos = [
            (1, None, "2026-11-01"), (2, None, "2026-11-02"),
            (3, None, "2026-10-04"), (4, "same-day", "2026-11-01"),
            (5, "A", "2026-09-01"), (6, "A", "2026-09-02"),
            (7, "B", "2026-09-01"), (8, "C", None), (9, "opposite", None),
        ]
        result = self.service.execute({"cmd": "suggest_folders", "asset_ids": [1, 2, 17], "photos": photos})
        self.assertEqual([folder["folder"] for folder in result["folders"]], ["A", "B", "C"])
        pending = self.service.execute({"cmd": "suggest_folders", "asset_ids": [1, 2], "photos": photos, "resolve_days": True})
        self.assertEqual(pending["missing_days"], [8])
        expected = [2 * 0.75**4, 0.85**4, 0.45**4]
        np.testing.assert_allclose([row["share"] for row in result["folders"]],
                                   np.array(expected) / sum(expected), atol=0.001)

    def test_catalogue_folder_cache_uses_modified_day_and_refreshes_after_edits(self):
        catalogue = self.root / "catalog.sqlite"
        with closing(sqlite3.connect(catalogue)) as connection:
            with connection:
                connection.execute("""CREATE TABLE assets(id INTEGER PRIMARY KEY,path TEXT,exif_date TEXT,
                                        file_modified TEXT,is_listed INTEGER,ext_lower TEXT)""")
                connection.executemany("INSERT INTO assets VALUES(?,?,?,?,1,'jpg')", [
                    (1, "F:/Incoming/a.jpg", "2026-11-01T12:00:00", None),
                    (2, "F:/Incoming/nested/c.jpg", None, None),
                    (3, "F:/Sorted/A/b.jpg", None, "2026-11-01T13:00:00"),
                    (4, "F:/Sorted/B/c.jpg", None, "2026-10-01T13:00:00"),
                ])
            set_vectors(self.service, {i: np.array([1, 0], dtype=np.float32) for i in range(1, 5)})
            self.service._cuda_torch = False
            request = {"cmd": "suggest_folders", "catalog": str(catalogue), "incoming": "F:/Incoming",
                       "extensions": ["jpg"], "asset_ids": [1], "resolve_days": True}
            result = self.service.execute(request)
            self.assertEqual(result, {"folders": [{"folder": r"F:\Sorted\B", "share": 1.0}]})
            cached = self.service._folder_metadata
            self.assertEqual(self.service.execute(request), result)
            self.assertIs(self.service._folder_metadata, cached)

            with connection:
                connection.execute("UPDATE assets SET path='F:/Sorted/C/c.jpg' WHERE id=4")
            self.assertEqual(self.service.execute(request), {"folders": [{"folder": r"F:\Sorted\C", "share": 1.0}]})
            self.assertIsNot(self.service._folder_metadata, cached)

            with connection:
                connection.execute("UPDATE assets SET file_modified=NULL WHERE id=4")
            self.assertEqual(self.service.execute(request)["missing_days"], [4])
            self.assertEqual(self.service.execute({**request, "days": {"4": "2026-11-01"}}), {"folders": []})

    def test_folder_suggestions_take_only_the_25_nearest_eligible_neighbours(self):
        values = {i: np.array([1 if i <= 26 else 0.99, 0], dtype=np.float32) for i in range(1, 29)}
        set_vectors(self.service, values)
        photos = [(1, None, None)] + [(i, "strong" if i <= 26 else "weak", None) for i in range(2, 29)]
        result = self.service.execute({"cmd": "suggest_folders", "asset_ids": [1], "photos": photos})
        self.assertEqual(result, {"folders": [{"folder": "strong", "share": 1.0}]})

    def test_face_aware_series_prefers_face_count_then_confidence(self):
        vector = np.array([1, 0], dtype=np.float32)
        self.series([(i, i) for i in range(1, 5)], {i: vector for i in range(1, 5)},
                    {1: 0.9, 2: 0.7, 3: 0.6, 4: 0.5})
        with self.service.faces_db:
            self.service.faces_db.executemany(
                "INSERT INTO faces(asset_id,det_score,source,state) VALUES(?,?,'ai','unknown')",
                [(2, 0.99), (3, 0.85), (3, 0.85), (4, 0.90), (4, 0.90)],
            )
        request = {"cmd": "series", "items": [{"asset_id": i, "t": i} for i in range(1, 5)],
                   "max_gap": 10, "min_sim": 0.90}
        self.assertEqual(self.service.execute(request)["groups"][0]["best"], 1)
        self.assertEqual(self.service.execute({**request, "face_aware": True})["groups"][0]["best"], 4)

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

    def cluster_fixture(self, east=10, west=10, noise=2):
        vectors = {}
        for aid in range(1, east + 1): vectors[aid] = np.array([1.0, 0.0, 0.0], dtype=np.float32)
        for aid in range(east + 1, east + west + 1): vectors[aid] = np.array([0.0, 1.0, 0.0], dtype=np.float32)
        for aid in range(east + west + 1, east + west + noise + 1): vectors[aid] = np.array([0.0, 0.0, 1.0], dtype=np.float32)
        set_vectors(self.service, vectors)
        return vectors

    def test_clusters_separate_blobs_and_exclude_labeled_assets(self):
        self.cluster_fixture()
        self.service._labels["category"] = {1: {99}}
        result = self.service.execute({"cmd": "clusters", "min_size": 8, "min_sim": 0.72})
        groups = result["groups"]
        self.assertEqual(result["unlabeled"], 21)
        self.assertEqual([group["total"] for group in groups], [10, 9])
        self.assertEqual({frozenset(group["ids"]) for group in groups}, {frozenset(range(2, 11)), frozenset(range(11, 21))})
        self.assertNotIn(1, [aid for group in groups for aid in group["ids"]])

    def test_clusters_min_size_filters_small_blob(self):
        self.cluster_fixture(east=8, west=7, noise=2)
        result = self.service.execute({"cmd": "clusters", "min_size": 8})
        self.assertEqual([group["total"] for group in result["groups"]], [8])

    def test_cluster_ignore_and_reset(self):
        self.cluster_fixture()
        first = self.service.execute({"cmd": "clusters", "min_size": 8})
        ignored_ids = first["groups"][0]["ids"]
        self.assertEqual(self.service.execute({"cmd": "cluster_ignore", "asset_ids": ignored_ids})["ignored"], 1)
        filtered = self.service.execute({"cmd": "clusters", "min_size": 8})
        self.assertEqual(len(filtered["groups"]), 1)
        self.assertEqual(self.service.execute({"cmd": "cluster_ignore_reset"},)["ignored"], 0)
        self.assertEqual(len(self.service.execute({"cmd": "clusters", "min_size": 8})["groups"]), 2)

    def test_cluster_output_is_deterministic(self):
        self.cluster_fixture()
        first = self.service.execute({"cmd": "clusters", "min_size": 8})
        second = self.service.execute({"cmd": "clusters", "min_size": 8})
        self.assertEqual(first, second)

    def test_search_cutoff_and_decode_only_color_pass(self):
        item = self.make_file("east-colors.jpg")
        self.index([item]); self.wait_idle()
        self.assertEqual(self.index([item])['queued'], 0)
        self.service.db.execute("DELETE FROM colors WHERE asset_id=?", (item["asset_id"],)); self.service.db.commit()
        self.assertEqual(self.index([item])['queued'], 1)
        queued = self.service._queue[0]
        self.assertTrue(queued["colors_only"])
        self.service._queue.clear()
        self.assertEqual(self.service.execute({"cmd": "search_text", "query": "unrelated", "limit": 5})["results"], [])

    def test_relative_text_cutoff_keeps_low_probability_true_matches(self):
        self.assertAlmostEqual(text_probability_cutoff(0.39), 0.0195)
        self.assertAlmostEqual(text_probability_cutoff(0.015), 0.003)
        probs = [0.39, 0.16, 0.074, 0.015, 0.006, 0.0006]
        cutoff = text_probability_cutoff(max(probs))
        self.assertEqual([p for p in probs if p >= cutoff], [0.39, 0.16, 0.074])
        floor_cutoff = text_probability_cutoff(0.015)
        self.assertEqual([p for p in probs if p >= floor_cutoff], [0.39, 0.16, 0.074, 0.015, 0.006])

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
        self.assertEqual(set(self.service._index_snapshot()[0]), {first["asset_id"], second["asset_id"]})

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
        vectors = {}
        ids = [item["asset_id"] for item in items]
        vectors[ids[0]] = np.array([1.0, 0.0] + [0.0] * (DIMENSION - 2), dtype=np.float32)
        vectors[ids[1]] = np.array([0.9, (1 - 0.9 ** 2) ** 0.5] + [0.0] * (DIMENSION - 2), dtype=np.float32)
        vectors[ids[2]] = np.array([0.8, 0.6] + [0.0] * (DIMENSION - 2), dtype=np.float32)
        vectors[ids[3]] = np.array([0.7, (1 - 0.7 ** 2) ** 0.5] + [0.0] * (DIMENSION - 2), dtype=np.float32)
        vectors[ids[4]] = np.array([0.95, (1 - 0.95 ** 2) ** 0.5] + [0.0] * (DIMENSION - 2), dtype=np.float32)
        set_vectors(self.service, vectors)
        set_result = self.service.execute({"cmd": "set_labels", "kind": "category",
            "pairs": [[ids[0], 30], [ids[1], 20], [ids[1], 40], [ids[2], 50], [ids[3], 30], [ids[3], 40]],
            "parents": {"30": 20, "20": 10, "10": None, "40": None, "50": None}})
        self.assertEqual(set_result, {"assets": 4, "assignments": 6, "labels": 5})
        result = self.service.execute({"cmd": "suggest", "kind": "category", "asset_ids": [ids[0], ids[4]], "limit": 5})
        rows = result["results"][str(ids[0])]
        self.assertNotIn(ids[0], result["results"])
        self.assertEqual([40, 50], [row["label_id"] for row in rows])
        quantized = np.asarray([0.9, 0.8, 0.7], dtype=np.float16).astype(np.float32)
        self.assertAlmostEqual(float((quantized[0] + quantized[2]) / quantized.sum()), rows[0]["score"], places=5)
        self.assertEqual(2, rows[0]["support"])
        self.assertNotIn(30, [row["label_id"] for row in rows])
        self.assertNotIn(20, [row["label_id"] for row in rows])
        self.assertEqual(2, result["combined"][0]["count"])

    def test_suggest_batch_grouping_unlabeled_and_unknown_kind(self):
        items = [self.make_file(f"batch-{name}.jpg") for name in ("a", "b", "c", "d")]
        self.index(items); self.wait_idle()
        ids = [item["asset_id"] for item in items]
        vectors = {}
        for aid, x in zip(ids, (1.0, 0.95, 0.9, 0.85)):
            vectors[aid] = np.array([x, (1 - x * x) ** 0.5] + [0.0] * (DIMENSION - 2), dtype=np.float32)
        set_vectors(self.service, vectors)
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
        set_vectors(self.service, dict(zip([101, 102, 103, 104], vectors)))
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
        set_vectors(self.service, dict(zip([201, 202, 203], vectors)))
        result = self.service.execute({"cmd": "order_by_similarity", "asset_ids": [201, 999, 203, 202]})["asset_ids"]
        self.assertEqual(result[-1], 999)
        self.assertLessEqual(abs(result.index(201) - result.index(202)), 1)


if __name__ == "__main__":
    unittest.main()
