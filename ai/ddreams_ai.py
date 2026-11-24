"""Persistent local SigLIP2 JSON-lines service for DDreams."""

from __future__ import annotations

import concurrent.futures
from collections import deque
from contextlib import closing
from datetime import date
import ctypes
import gc
import hashlib
import io
import json
import ntpath
import os
import sqlite3
import sys
import threading
import time
import traceback
from pathlib import Path
from typing import Any, Callable

import numpy as np

try:
    from pillow_heif import register_heif_opener
except ImportError:
    pass
else:
    register_heif_opener(thumbnails=False)

# The server is started in isolated mode (-I), which leaves its own folder off sys.path.
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from faces_schema import ensure_faces_schema  # noqa: E402

MODEL_NAME = "ViT-L-16-SigLIP2-256"
DIMENSION = 1024
SCHEMA_VERSION = "1"
COLOR_VERSION = 1
QUALITY_VERSION = 1
DECODER_VERSION = 2
IMAGE_RETRY_EXTENSIONS = frozenset({"heic", "heif", "hif", "avif", "jp2", "jpc", "j2k"})
RAW_EXTENSIONS = frozenset({
    "3fr", "arq", "arw", "bay", "cap", "cr2", "cr3", "crw", "cs1", "dcr", "dcs",
    "dng", "drf", "eip", "erf", "fff", "gpr", "iiq", "k25", "kdc", "mdc", "mef",
    "mos", "mrw", "nef", "nrw", "orf", "ori", "pef", "ptx", "pxn", "r3d", "raf",
    "raw", "rw2", "rwl", "rwz", "sr2", "srf", "srw", "x3f",
})
RAW_RETRY_ERRORS = frozenset({
    "LibRawNoThumbnailError", "LibRawUnsupportedThumbnailError", "UnidentifiedImageError",
})
RAW_MIN_EMBEDDED_PX = 800
FACE_VERSION = 1
FACE_IMAGE_MAX = 2048
FACE_MIN_SIDE = 28
FACE_MATCH_SIM = 0.45
FACE_STRONG_SIM = 0.60
FACE_TOP_K = 5
FACE_CLUSTER_MAX = 60000
FACE_CLUSTER_SIM = 0.55
FACE_CLUSTER_MIN_SIZE = 4
FACE_CLUSTER_GUESS_SIM = 0.35
FACE_TEMPLATE = np.array([[38.2946, 51.6963], [73.5318, 51.5014], [56.0252, 71.7366], [41.5493, 92.3655], [70.7299, 92.2041]], dtype=np.float64)
TEXT_PROB_FLOOR = 0.003
TEXT_PROB_RELATIVE = 0.05
SIMILARITY_COSINE_FLOOR = 0.60
SUGGEST_K = 10
MIN_SCORE = 0.30
AUTO_GROUP_SCORE = 0.70
DUP_SAME = 0.95  # candidate level; the app keeps only same capture time / base name
DUP_SIMILAR = 0.93
SUGGEST_BATCH_CHUNK = 128
CLUSTER_MIN_SIZE = 8
CLUSTER_MIN_SIM = 0.72
CLUSTER_IGNORE_SIM = 0.95  # centroids of related groups easily reach 0.90
CLUSTER_ITERATIONS = 12
MAP_ITERATIONS = 15
MAP_RADIUS_SCALE = 0.94  # share of the packed circle that photos fill
MAP_PACK_DENSITY = 0.55
MAP_PACK_GAP = 1.04
MAP_PACK_ITERATIONS = 400
MAP_TSNE_ITERATIONS = 600
MAP_TSNE_EXAGGERATION = 150
MAP_TSNE_SEED = 7
MAP_GOLDEN_ANGLE = 2.39996323
CLUSTER_VOCABULARY = tuple(dict.fromkeys((
    "krajina", "hory", "kopce", "údolie", "les", "stromy", "lúka", "pole", "záhrada", "park",
    "more", "pláž", "pobrežie", "ostrov", "útes", "rieka", "jazero", "vodopád", "prameň", "močiar",
    "západ slnka", "východ slnka", "obloha", "oblaky", "dúha", "búrka", "blesk", "hmla", "noc", "hviezdy",
    "sneh", "zima", "jar", "leto", "jeseň", "ľad", "voda", "oheň", "púšť", "skaly",
    "mesto", "dedina", "ulica", "námestie", "architektúra", "kostol", "hrad", "zámok", "most", "veža",
    "interiér", "dom", "byt", "izba", "kuchyňa", "múzeum", "galéria", "pamiatky", "ruiny", "schody",
    "portrét", "deti", "rodina", "svadba", "oslava", "stretnutie", "ľudia", "skupina", "silueta", "móda",
    "koncert", "hudba", "festival", "divadlo", "umenie", "tanec", "šport", "futbal", "cyklistika", "beh",
    "pes", "mačka", "vtáky", "dravce", "kačice", "zvieratá", "kone", "hmyz", "motýle", "včely",
    "kvety", "ruže", "rastliny", "huby", "ovocie", "zelenina", "jedlo", "dezert", "káva", "víno",
    "auto", "motocykel", "bicykel", "vlak", "lietadlo", "loď", "autobus", "doprava", "cestovanie", "kempovanie",
    "makro", "detail", "textúra", "abstrakcia", "čiernobiela fotografia", "odraz", "tieň", "svetlá", "ohnisko", "panoráma",
    "dokumenty", "snímky obrazovky", "technika", "počítač", "telefón", "knihy", "hračky", "nástroje", "nábytok", "značky",
    "počasie", "príroda", "zvieratká", "akvárium", "plazy", "zoo", "farmárstvo", "vinica", "olivový háj", "sad",
    "turistika", "lyžovanie", "plávanie", "vodné športy", "lezenie", "preteky", "medaily", "výlet", "stan", "karavan",
    "jedlo na tanieri", "pečivo", "ryby", "morské živočíchy", "lastúry", "kamene", "kvapky", "okno", "dvere", "fasáda",
    "fontána", "socha", "pomník", "cintorín", "trh", "obchod", "reštaurácia", "hotel", "letisko", "prístav",
    "silvester", "vianoce", "veľká noc", "narodeniny", "krst", "promócia", "školská fotografia", "pracovisko", "kancelária", "ulica v noci",
)))
KEYWORD_VOCABULARY = tuple(dict.fromkeys(CLUSTER_VOCABULARY + (
    "slon", "nosorožec", "hroch", "žirafa", "zebra", "lev", "tiger", "leopard",
    "gepard", "medveď", "vlk", "líška", "jeleň", "srna", "diviak", "zajac",
    "veverička", "opica", "gorila", "panda", "ťava", "lama", "ovca", "koza",
    "krava", "prasa", "páv", "sova", "orol", "sokol", "bocian", "volavka",
    "plameniak", "papagáj", "labuť", "hus", "holub", "tučniak", "vrabec", "sýkorka",
    "krokodíl", "had", "jašterica", "korytnačka", "žaba", "delfín", "veľryba", "žralok",
    "vážka", "motýlica", "modlivka", "pavúk", "pavučina", "lienka", "čmeliak",
    "osa", "mravec", "chrobák", "koník", "slimák", "húsenica", "mucha",
    "orchidea", "tulipán", "púpava", "sedmokráska", "slnečnica", "levanduľa", "lekno",
    "mäta", "papraď", "mach", "lišajník", "kaktus", "šiška", "lístie", "kôra stromu",
    "rosa", "námraza", "snehová vločka", "kvapka vody", "špliechanie vody", "dážď",
    "dúhové odlesky", "protisvetlo", "bokeh", "dlhá expozícia", "ohňostroj", "Mesiac",
    "ľadovec", "jaskyňa", "skalný oblúk", "hrebeň hôr", "duny", "savana", "džungľa",
    "lesná cesta", "potok", "rybník", "priehrada", "vinice", "alej", "kvitnúci strom",
    "maják", "železnica", "stanica", "tunel", "električka", "ulička", "mrakodrap",
    "okná", "vitráž", "historická budova", "kamenný múr", "drevenica", "nádvorie",
    "skleník", "jesenné lístie", "mraky", "nočná obloha", "vidiek", "panoráma mesta",
)))
BATCH_SIZE = 32
DECODE_THREADS = 6
CHROMATIC_COLORS = ("červená", "oranžová", "hnedá", "žltá", "zelená", "tyrkysová", "modrá", "fialová", "ružová")
NEUTRAL_COLORS = ("biela", "sivá", "čierna")
COLOR_PALETTE = CHROMATIC_COLORS + NEUTRAL_COLORS + ("čiernobiela",)
COLOR_DEFAULTS = {color: 0.45 if color in NEUTRAL_COLORS else 0.06 for color in COLOR_PALETTE}


def text_probability_cutoff(top_prob: float) -> float:
    return max(TEXT_PROB_FLOOR, TEXT_PROB_RELATIVE * top_prob)


def _hsv(rgb: np.ndarray) -> tuple[np.ndarray, np.ndarray, np.ndarray]:
    values = np.asarray(rgb, dtype=np.float32) / 255.0
    maximum, minimum = values.max(axis=2), values.min(axis=2)
    delta = maximum - minimum
    red, green, blue = values[..., 0], values[..., 1], values[..., 2]
    hue = np.zeros_like(maximum)
    nonzero = delta > 1e-6
    mask = nonzero & (maximum == red)
    hue[mask] = np.mod((green[mask] - blue[mask]) / delta[mask], 6)
    mask = nonzero & (maximum == green)
    hue[mask] = (blue[mask] - red[mask]) / delta[mask] + 2
    mask = nonzero & (maximum == blue)
    hue[mask] = (red[mask] - green[mask]) / delta[mask] + 4
    hue *= 60.0
    saturation = np.divide(delta, maximum, out=np.zeros_like(delta), where=maximum > 1e-6)
    return hue, saturation, maximum


def palette_counts(image: Any) -> dict[str, float]:
    """Measure shares on the indexer's decoded image (max 512 px), with no extra photo read."""
    if not hasattr(image, "convert"):
        return {}
    rgb = np.asarray(image.convert("RGB"), dtype=np.uint8)
    hue, saturation, value = _hsv(rgb)
    if float(saturation.mean()) <= 0.035:
        return {"čiernobiela": 1.0}
    warm = (hue >= 10) & (hue < 50)
    pale = warm & (saturation < 0.35) & (value >= 0.65)
    neutral = (saturation < 0.20) | (value <= 0.17) | pale
    brown = ~neutral & warm & ((value < 0.65) | (saturation < 0.50))
    light_red = (hue < 15) | (hue >= 345)
    pink = ~neutral & light_red & (saturation < 0.50) & (value >= 0.65)
    chroma = ~neutral & ~brown & ~pink
    masks = {
        "čierna": neutral & (value <= 0.17), "biela": neutral & (value >= 0.84),
        "sivá": neutral & (value > 0.17) & (value < 0.84), "hnedá": brown,
        "červená": chroma & light_red, "oranžová": chroma & (hue >= 15) & (hue < 40),
        "žltá": chroma & (hue >= 40) & (hue < 70), "zelená": chroma & (hue >= 70) & (hue < 155),
        "tyrkysová": chroma & (hue >= 155) & (hue < 195), "modrá": chroma & (hue >= 195) & (hue < 255),
        "fialová": chroma & (hue >= 255) & (hue < 290), "ružová": pink | (chroma & (hue >= 290) & (hue < 345)),
    }
    weights = np.where((saturation >= 0.60) & (value >= 0.30), 3.0, 1.0)
    return {name: float(weights[mask].sum()) / saturation.size for name, mask in masks.items() if mask.any()}


def dominant_colors(shares: dict[str, float], thresholds: dict[str, float] | None = None) -> list[str]:
    if "čiernobiela" in shares:
        return ["čiernobiela"]
    thresholds = thresholds or COLOR_DEFAULTS
    ranked = sorted(shares.items(), key=lambda item: (-item[1], COLOR_PALETTE.index(item[0])))
    selected = [name for name, share in ranked if share >= thresholds.get(name, COLOR_DEFAULTS[name])]
    return selected[:4] or [name for name, _ in ranked[:1]]


def learn_thresholds(samples: dict[str, list[tuple[float, bool]]]) -> dict[str, dict[str, float | int]]:
    learned = {}
    for color in COLOR_PALETTE:
        examples = samples.get(color, [])
        positives = sum(label for _, label in examples)
        negatives = len(examples) - positives
        default = COLOR_DEFAULTS[color]
        threshold = default
        eligible = len(examples) >= 5 and positives and negatives
        if eligible:
            values = sorted({max(0.0, min(1.0, share)) for share, _ in examples})
            candidates = [0.0, 0.70]
            candidates.extend((left + right) / 2 for left, right in zip(values, values[1:]))
            raw = max(candidates, key=lambda value: (
                (sum(score >= value for score, label in examples if label) / positives +
                 sum(score < value for score, label in examples if not label) / negatives) / 2,
                -abs(value - default)))
            raw = max(0.01, min(0.70, raw))
            threshold = max(0.01, min(0.70, default + (raw - default) * len(examples) / (len(examples) + 10)))
        learned[color] = {"default": default, "threshold": threshold, "examples": len(examples) if eligible else 0,
                          "positive": positives, "negative": negatives}
    return learned


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


def process_memory() -> dict[str, int] | None:
    """Windows RSS/private commit; avoids adding psutil to the runtime."""
    if os.name != "nt":
        return None
    class Counters(ctypes.Structure):
        _fields_ = [("cb", ctypes.c_ulong), ("PageFaultCount", ctypes.c_ulong),
                    ("PeakWorkingSetSize", ctypes.c_size_t), ("WorkingSetSize", ctypes.c_size_t),
                    ("QuotaPeakPagedPoolUsage", ctypes.c_size_t), ("QuotaPagedPoolUsage", ctypes.c_size_t),
                    ("QuotaPeakNonPagedPoolUsage", ctypes.c_size_t), ("QuotaNonPagedPoolUsage", ctypes.c_size_t),
                    ("PagefileUsage", ctypes.c_size_t), ("PeakPagefileUsage", ctypes.c_size_t),
                    ("PrivateUsage", ctypes.c_size_t)]
    try:
        psapi = ctypes.WinDLL("psapi", use_last_error=True)
        kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        kernel.GetCurrentProcess.restype = ctypes.c_void_p
        psapi.GetProcessMemoryInfo.argtypes = (ctypes.c_void_p, ctypes.c_void_p, ctypes.c_ulong)
        psapi.GetProcessMemoryInfo.restype = ctypes.c_int
        counters = Counters()
        counters.cb = ctypes.sizeof(counters)
        if not psapi.GetProcessMemoryInfo(kernel.GetCurrentProcess(), ctypes.byref(counters), counters.cb):
            return None
        return {"working_set": int(counters.WorkingSetSize), "private": int(counters.PrivateUsage)}
    except Exception:
        return None


def _decode_raw(path: str, decode_side: int):
    from PIL import Image, ImageOps
    import rawpy

    # Files with a RAW suffix can contain ordinary images. Preserve errors such as
    # LibRawTooBigError so the indexing worker can record them.
    try:
        raw_file = rawpy.imread(path)
    except rawpy.LibRawFileUnsupportedError:
        return None

    min_preview = min(decode_side, RAW_MIN_EMBEDDED_PX)
    with raw_file as raw:
        try:
            thumb = raw.extract_thumb()
        except (rawpy.LibRawNoThumbnailError, rawpy.LibRawUnsupportedThumbnailError):
            thumb = None

        image = None
        if thumb is not None and thumb.format == rawpy.ThumbFormat.JPEG:
            try:
                with Image.open(io.BytesIO(thumb.data)) as preview:
                    if max(preview.size) >= min_preview:
                        preview.draft("RGB", (decode_side, decode_side))
                        image = ImageOps.exif_transpose(preview).convert("RGB")
            except (OSError, ValueError, SyntaxError, Image.DecompressionBombError):
                pass
        elif thumb is not None and thumb.format == rawpy.ThumbFormat.BITMAP:
            pixels = thumb.data
            if pixels.ndim == 3 and pixels.shape[2] == 3 and max(pixels.shape[:2]) >= min_preview:
                image = Image.fromarray(pixels, "RGB")
            del pixels
        # Release the preview data before allocating the developed image.
        del thumb
        if image is None:
            image = Image.fromarray(raw.postprocess(half_size=True, use_camera_wb=True, output_bps=8))
    image.thumbnail((decode_side, decode_side), Image.Resampling.BICUBIC)
    return image


def decode_photo(item: dict[str, Any], max_side: int = 512):
    """Decode a photo to an oriented RGB PIL image suitable for model input."""
    from PIL import Image, ImageOps

    path = item["path"]
    ext = Path(path).suffix.lower().lstrip(".")
    decode_side = max(1024, max_side)
    if ext in RAW_EXTENSIONS:
        image = _decode_raw(path, decode_side)
        if image is not None:
            return _finish_decoded(image, max_side)

    with Image.open(path) as source:
        if ext in {"jpg", "jpeg", "png"} or (ext in RAW_EXTENSIONS and source.format in {"JPEG", "PNG"}):
            source.draft("RGB", (decode_side, decode_side))
        if ext in {"tif", "tiff"} or (ext in RAW_EXTENSIONS and source.format == "TIFF"):
            source.thumbnail((decode_side, decode_side), Image.Resampling.BICUBIC)
        image = ImageOps.exif_transpose(source).convert("RGB")
    return _finish_decoded(image, max_side)


def _finish_decoded(image: Any, max_side: int = 512):
    """Measure quality while the image is still up to 1024 px, then reduce it to the model input size."""
    from PIL import Image

    quality = compute_quality(image)
    image.thumbnail((max_side, max_side), Image.Resampling.BICUBIC)
    image.info["ddreams_quality"] = quality
    return image


def compute_quality(image: Any) -> tuple[float, float, float, float]:
    """Return sharpness, clipped highlights/shadows and normalized mean luma."""
    from PIL import Image

    rgb_image = image.convert("RGB")
    if max(rgb_image.size) > 1024:
        scale = 1024 / max(rgb_image.size)
        rgb_image = rgb_image.resize((max(1, round(rgb_image.width * scale)), max(1, round(rgb_image.height * scale))), Image.Resampling.BILINEAR)
    rgb = np.asarray(rgb_image, dtype=np.float32)
    luma = rgb[:, :, 0] * np.float32(0.299) + rgb[:, :, 1] * np.float32(0.587) + rgb[:, :, 2] * np.float32(0.114)
    padded = np.pad(luma, 1, mode="edge")
    laplacian = padded[:-2, 1:-1] + padded[2:, 1:-1] + padded[1:-1, :-2] + padded[1:-1, 2:] - 4 * luma
    tile_variances = []
    for ys in np.array_split(np.arange(luma.shape[0]), 12):
        for xs in np.array_split(np.arange(luma.shape[1]), 12):
            tile_variances.append(float(np.var(laplacian[np.ix_(ys, xs)])) if len(ys) and len(xs) else 0.0)
    total = luma.size
    return float(np.mean(sorted(tile_variances)[-3:])), float(np.count_nonzero(luma >= 250) / total), float(np.count_nonzero(luma <= 5) / total), float(np.mean(luma, dtype=np.float64) / 255.0)


def face_detect(img: Any, det: Any, det_in: str, size: int = 640, thr: float = 0.5):
    """SCRFD decode and greedy NMS; coordinates are in the supplied image."""
    from PIL import Image
    w, h = img.size
    scale = min(size / w, size / h)
    nw, nh = max(1, int(w * scale)), max(1, int(h * scale))
    canvas = np.zeros((size, size, 3), dtype=np.float32)
    canvas[:nh, :nw] = np.asarray(img.resize((nw, nh), Image.Resampling.BILINEAR), dtype=np.float32)
    blob = ((canvas - 127.5) / 128.0).transpose(2, 0, 1)[None]
    outs = det.run(None, {det_in: blob})
    boxes, scores, kpss = [], [], []
    for i, stride in enumerate((8, 16, 32)):
        sc = outs[i].reshape(-1); bb = outs[i + 3].reshape(-1, 4) * stride; kp = outs[i + 6].reshape(-1, 10) * stride
        n = size // stride
        cy, cx = np.mgrid[:n, :n]
        centers = np.stack([cx, cy], -1).reshape(-1, 2).astype(np.float32) * stride
        centers = np.repeat(centers, 2, axis=0)
        keep = sc >= thr
        if not keep.any():
            continue
        c = centers[keep]; b = bb[keep]; k = kp[keep].reshape(-1, 5, 2)
        boxes.append(np.stack([c[:, 0] - b[:, 0], c[:, 1] - b[:, 1], c[:, 0] + b[:, 2], c[:, 1] + b[:, 3]], 1))
        scores.append(sc[keep]); kpss.append(c[:, None, :] + k)
    if not boxes:
        return np.zeros((0, 4)), np.zeros(0), np.zeros((0, 5, 2))
    boxes = np.concatenate(boxes) / scale; scores = np.concatenate(scores); kpss = np.concatenate(kpss) / scale
    order = np.argsort(-scores, kind="stable"); picked = []
    while len(order):
        i = int(order[0]); picked.append(i)
        if len(order) == 1:
            break
        rest = order[1:]
        xx1 = np.maximum(boxes[i, 0], boxes[rest, 0]); yy1 = np.maximum(boxes[i, 1], boxes[rest, 1])
        xx2 = np.minimum(boxes[i, 2], boxes[rest, 2]); yy2 = np.minimum(boxes[i, 3], boxes[rest, 3])
        inter = np.maximum(0, xx2 - xx1) * np.maximum(0, yy2 - yy1)
        area_i = max(0, boxes[i, 2] - boxes[i, 0]) * max(0, boxes[i, 3] - boxes[i, 1])
        area_r = np.maximum(0, boxes[rest, 2] - boxes[rest, 0]) * np.maximum(0, boxes[rest, 3] - boxes[rest, 1])
        iou = inter / np.maximum(area_i + area_r - inter, 1e-12)
        order = rest[iou <= 0.4]
    picked = np.asarray(picked, dtype=np.int64)
    return boxes[picked], scores[picked], kpss[picked]


def face_align(img: Any, kps: np.ndarray):
    from PIL import Image
    src = kps.astype(np.float64); dst = FACE_TEMPLATE
    ms, md = src.mean(0), dst.mean(0); a, b = src - ms, dst - md
    cov = b.T @ a / 5; U, S, Vt = np.linalg.svd(cov); d = np.ones(2)
    if np.linalg.det(cov) < 0: d[1] = -1
    R = U @ np.diag(d) @ Vt; scale = (S * d).sum() / (a ** 2).sum() * 5
    M = np.eye(3); M[:2, :2] = scale * R; M[:2, 2] = md - scale * R @ ms
    inv = np.linalg.inv(M)
    return img.transform((112, 112), Image.AFFINE, tuple(inv[:2].reshape(-1)), resample=Image.Resampling.BILINEAR)


def _box_iou(a: tuple[float, float, float, float], b: tuple[float, float, float, float]) -> float:
    x1, y1 = max(a[0], b[0]), max(a[1], b[1]); x2, y2 = min(a[0] + a[2], b[0] + b[2]), min(a[1] + a[3], b[1] + b[3])
    inter = max(0.0, x2 - x1) * max(0.0, y2 - y1)
    return inter / max(a[2] * a[3] + b[2] * b[3] - inter, 1e-12)


class SiglipEmbedder:
    """Real model adapter. Searches read the service-owned vector index."""

    dimension = DIMENSION
    device = "cuda"

    def __init__(self, model_dir: str | os.PathLike[str]):
        os.environ["HF_HUB_OFFLINE"] = "1"
        import torch
        import open_clip
        from safetensors import safe_open
        from open_clip.tokenizer import HFTokenizer

        self.torch = torch
        self._lock = threading.RLock()
        # The meta-device context matters: device="meta" alone still builds random CPU
        # weights first (~5 GB private memory, ~35 s). Measured 2026-11-08.
        with torch.device("meta"):
            self.model, _, self.preprocess = open_clip.create_model_and_transforms(
                MODEL_NAME, device="meta", image_mean=(0.5, 0.5, 0.5), image_std=(0.5, 0.5, 0.5),
                image_interpolation="bicubic", image_resize_mode="squash",
            )
        checkpoint = Path(model_dir) / "open_clip_model.fp16.safetensors"
        # Tensor by tensor from the CPU memory map: safetensors' bulk load_file fails with
        # "invalid python storage" when commit memory is short, and loading straight to
        # CUDA keeps ~0.5 GB extra private memory.
        with safe_open(str(checkpoint), framework="pt", device="cpu") as weights:
            state = {key: weights.get_tensor(key).to("cuda") for key in weights.keys()}
        self.model.load_state_dict(state, strict=False, assign=True)
        del state
        gc.collect()
        torch.cuda.empty_cache()
        self.model.eval()
        self._logit_scale = self.model.logit_scale.detach().float().exp()
        self._logit_bias = self.model.logit_bias.detach().float()
        config = open_clip.get_model_config(MODEL_NAME)
        text_cfg = config["text_cfg"]
        kwargs = text_cfg.get("tokenizer_kwargs", {})
        self.tokenizer = HFTokenizer(
            str(model_dir), context_length=int(text_cfg["context_length"]),
            clean=kwargs.get("clean", "canonicalize"),
        )

    def encode_images(self, images: list[Any]) -> np.ndarray:
        torch = self.torch
        batch = torch.stack([self.preprocess(image) for image in images])
        with self._lock:
            batch = batch.to("cuda", non_blocking=True)
            with torch.inference_mode(), torch.autocast("cuda"):
                features = self.model.encode_image(batch)
                features = features / features.norm(dim=-1, keepdim=True).clamp_min(1e-12)
            return features.float().cpu().numpy()

    def _text_vector(self, query: str):
        return self._encode_texts([query])

    def _encode_texts(self, texts: list[str]):
        with self._lock:
            tokens = self.tokenizer(texts).to("cuda")
            with self.torch.inference_mode(), self.torch.autocast("cuda"):
                vector = self.model.encode_text(tokens)
                vector = vector / vector.norm(dim=-1, keepdim=True).clamp_min(1e-12)
            return vector.to(dtype=self.torch.float16)

    def search_text(self, query: str, limit: int, index) -> dict[str, Any]:
        ids, matrix, _ = index
        with self._lock:
            if not ids:
                return {"results": [], "searched": 0}
            scores = matrix @ self._text_vector(query).reshape(-1)
            probs = self.torch.sigmoid(self._logit_scale * scores.float() + self._logit_bias)
            top_prob = float(probs.max().item())
            cutoff = text_probability_cutoff(top_prob)
            values, positions = self.torch.topk(scores, len(ids))
            result = []
            for score, position in zip(values.cpu().tolist(), positions.cpu().tolist()):
                prob = float(probs[position].item())
                if prob >= cutoff:
                    result.append({"asset_id": ids[position], "score": float(score), "prob": prob})
                    if len(result) >= limit:
                        break
            return {"results": result, "searched": len(ids)}

    def similar(self, asset_id: int, limit: int, index) -> dict[str, Any]:
        ids, matrix, positions = index
        with self._lock:
            pos = positions[asset_id]
            scores = matrix @ matrix[pos]
            scores[pos] = -2.0
            probs = self.torch.sigmoid(self._logit_scale * scores.float() + self._logit_bias)
            values, chosen = self.torch.topk(scores, min(limit, max(0, len(ids) - 1)))
            result = [{"asset_id": ids[i], "score": score, "prob": float(probs[i].item())}
                      for score, i in zip(values.cpu().tolist(), chosen.cpu().tolist()) if score >= SIMILARITY_COSINE_FLOOR]
            return {"results": result, "searched": len(ids)}


class VectorIndex:
    """Contiguous float16 vectors; snapshots keep their backing allocation alive."""
    def __init__(self, ids: list[int], vectors: np.ndarray):
        values = np.asarray(vectors, dtype=np.float16)
        self.dimension = values.shape[1]
        self.count = len(ids)
        self.capacity = max(self.count, 1)
        self.matrix = np.empty((self.capacity, self.dimension), dtype=np.float16)
        self.matrix[:self.count] = values
        self.ids = list(map(int, ids))
        self.positions = {aid: i for i, aid in enumerate(self.ids)}
        self._torch = None
        self._device_matrix = None

    def snapshot(self):
        return self.ids, self.matrix[:self.count], self.positions

    def cuda_snapshot(self):
        if self._device_matrix is not None:
            return self.ids, self._device_matrix[:self.count], self.positions
        return None

    def upsert(self, ids: list[int], vectors: np.ndarray) -> None:
        missing = sum(int(aid) not in self.positions for aid in ids)
        required = self.count + missing
        if required > self.capacity:
            self.capacity = max(required, self.capacity * 2)
            matrix = np.empty((self.capacity, self.dimension), dtype=np.float16)
            matrix[:self.count] = self.matrix[:self.count]
            self.matrix = matrix
            if self._device_matrix is not None:
                device_matrix = self._device_matrix.new_empty(matrix.shape)
                device_matrix[:self.count].copy_(self._device_matrix[:self.count])
                self._device_matrix = device_matrix
        elif any(int(aid) in self.positions for aid in ids):
            # Existing snapshots may be read by another query.
            self.matrix = self.matrix.copy()
            if self._device_matrix is not None:
                self._device_matrix = self._device_matrix.clone()
        # Copy the ID structures so existing snapshots retain their mapping.
        self.ids = self.ids.copy()
        self.positions = self.positions.copy()
        updated_positions = []
        for aid, vector in zip(ids, vectors):
            aid = int(aid)
            pos = self.positions.get(aid)
            if pos is None:
                pos = self.count
                self.count += 1
                self.positions[aid] = pos
                self.ids.append(aid)
            self.matrix[pos] = vector
            updated_positions.append(pos)
        if self._device_matrix is not None and updated_positions:
            torch, device = self._torch, self._device_matrix.device
            updated_positions = list(dict.fromkeys(updated_positions))
            positions = torch.as_tensor(updated_positions, device=device)
            values = torch.as_tensor(self.matrix[updated_positions], device=device)
            self._device_matrix.index_copy_(0, positions, values)

    def remove(self, ids: set[int]) -> None:
        keep = [i for i, aid in enumerate(self.ids) if aid not in ids]
        if len(keep) == self.count:
            return
        self.ids = [self.ids[i] for i in keep]
        self.count = len(keep)
        self.capacity = max(self.count, 1)
        matrix = np.empty((self.capacity, self.dimension), dtype=np.float16)
        matrix[:self.count] = self.matrix[keep]
        self.matrix = matrix
        if self._device_matrix is not None:
            positions = self._torch.as_tensor(keep, device=self._device_matrix.device, dtype=self._torch.long)
            device_matrix = self._device_matrix.new_empty(matrix.shape)
            device_matrix[:self.count].copy_(self._device_matrix.index_select(0, positions))
            self._device_matrix = device_matrix
        self.positions = {aid: i for i, aid in enumerate(self.ids)}


class AIService:
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

    def _emit_indexed_assets(self) -> None:
        with self._condition:
            if (not self._newly_indexed_ids or self._active or self._queue or self._quality_queue
                    or self._face_scan_queue or self._face_active_ids):
                return
            ids = sorted(self._newly_indexed_ids)
            self._newly_indexed_ids.clear()
        self._emit({"event": "indexed_assets", "asset_ids": ids})

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

    def _wait_model(self) -> Any:
        self._model_ready.wait()
        if self._model_error:
            raise RequestError(self._model_error)
        if self._stop:
            raise RequestError("Server sa vypína")
        return self._embedder

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

    @staticmethod
    def _kind(value: Any) -> str:
        if value not in ("category", "keyword"):
            raise RequestError("kind musí byť category alebo keyword")
        return value

    def _set_labels(self, request: dict[str, Any]) -> dict[str, Any]:
        kind = self._kind(request.get("kind"))
        pairs, raw_parents = request.get("pairs"), request.get("parents", {})
        if not isinstance(pairs, list) or not isinstance(raw_parents, dict):
            raise RequestError("pairs musí byť zoznam a parents objekt")
        labels: dict[int, set[int]] = {}
        for pair in pairs:
            if not isinstance(pair, list) or len(pair) != 2 or any(isinstance(x, bool) or not isinstance(x, int) or x <= 0 for x in pair):
                raise RequestError("Každá dvojica labels musí obsahovať dve kladné ID")
            labels.setdefault(pair[0], set()).add(pair[1])
        parents: dict[int, int | None] = {}
        try:
            for label_id, parent_id in raw_parents.items():
                key = int(label_id)
                if key <= 0 or isinstance(parent_id, bool) or (parent_id is not None and (not isinstance(parent_id, int) or parent_id <= 0)):
                    raise ValueError
                parents[key] = parent_id
        except (TypeError, ValueError):
            raise RequestError("parents obsahuje neplatné ID") from None
        with self._lock:
            self._labels[kind] = labels
            self._parents[kind] = parents
        return {"assets": len(labels), "assignments": sum(map(len, labels.values())), "labels": len(parents)}

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

    def _similar_probability(self, score: float) -> float | None:
        if not self._similar_logits_checked:
            with self._lock:
                if not self._similar_logits_checked:
                    try:
                        from safetensors import safe_open
                        with safe_open(str(Path(self.model_dir) / "open_clip_model.fp16.safetensors"), framework="numpy") as weights:
                            scale = float(np.exp(np.float32(weights.get_tensor("logit_scale").item())))
                            bias = float(weights.get_tensor("logit_bias").item())
                        self._similar_logits = (scale, bias)
                    except Exception:
                        pass
                    self._similar_logits_checked = True
        if self._similar_logits is None:
            return None
        scale, bias = self._similar_logits
        value = np.float32(scale) * np.float32(score) + np.float32(bias)
        return float(1.0 / (1.0 + np.exp(-np.clip(value, -80, 80))))

    @staticmethod
    def _ancestors(label_id: int, parents: dict[int, int | None]) -> set[int]:
        found, current = set(), label_id
        while current in parents and parents[current] is not None:
            current = int(parents[current])
            if current in found:
                break
            found.add(current)
        return found

    def _top_neighbors(self, query_id: int, candidates: list[int], k: int,
                       index: tuple[list[int], Any, dict[int, int]]) -> list[tuple[int, float]]:
        ids, matrix, positions = index
        if query_id not in positions or not candidates:
            return []
        if isinstance(matrix, np.ndarray):
            query = matrix[positions[query_id]].astype(np.float32)
            sims = (matrix[[positions[aid] for aid in candidates]].astype(np.float32) @ query).astype(np.float16).astype(np.float32)
            try:
                sims[candidates.index(query_id)] = -np.inf
            except ValueError:
                pass
            take = min(k, len(candidates))
            chosen = np.argpartition(-sims, take - 1)[:take] if take else np.empty((0,), dtype=np.int64)
            chosen = chosen[np.argsort(-sims[chosen], kind="stable")]
            return [(candidates[int(i)], float(sims[i])) for i in chosen if np.isfinite(sims[i])]
        else:
            torch = self._cuda_torch
            candidate_pos = torch.as_tensor([positions[a] for a in candidates], device=matrix.device)
            with torch.inference_mode():
                scores = matrix[candidate_pos] @ matrix[positions[query_id]]
                scores = scores.masked_fill(candidate_pos == positions[query_id], -2.0)
                values, chosen = torch.topk(scores, min(k, len(candidates)))
                return [(candidates[int(i)], float(score)) for score, i in zip(values.cpu().tolist(), chosen.cpu().tolist()) if score > -1.5]

    def _rank_labels(self, query_id: int, kind: str, limit: int,
                     index: tuple[list[int], Any, dict[int, int]], labels: dict[int, set[int]],
                     parents: dict[int, int | None], min_score: float = MIN_SCORE) -> list[dict[str, Any]]:
        candidates = [aid for aid, values in labels.items() if values and aid in index[2]]
        neighbors = self._top_neighbors(query_id, candidates, SUGGEST_K, index)
        return self._rank_neighbors(query_id, neighbors, limit, labels, parents, min_score)

    @staticmethod
    def _rank_neighbors(query_id: int, neighbors: list[tuple[int, float]], limit: int,
                        labels: dict[int, set[int]], parents: dict[int, int | None],
                        min_score: float = MIN_SCORE) -> list[dict[str, Any]]:
        denominator = sum(score for _, score in neighbors)
        if not neighbors or denominator <= 0:
            return []
        excluded = set(labels.get(query_id, set()))
        for label_id in tuple(excluded):
            excluded.update(AIService._ancestors(label_id, parents))
        votes: dict[int, float] = {}
        support: dict[int, int] = {}
        for aid, similarity in neighbors:
            for label_id in labels[aid]:
                votes[label_id] = votes.get(label_id, 0.0) + similarity
                support[label_id] = support.get(label_id, 0) + 1
        ranked = [{"label_id": label_id, "score": score / denominator, "support": support[label_id]}
                  for label_id, score in votes.items() if label_id not in excluded and score / denominator >= min_score]
        ranked.sort(key=lambda row: (-row["score"], -row["support"], row["label_id"])); return ranked[:limit]

    def _suggest(self, request: dict[str, Any]) -> dict[str, Any]:
        kind = self._kind(request.get("kind"))
        asset_ids = request.get("asset_ids")
        if not isinstance(asset_ids, list) or any(isinstance(a, bool) or not isinstance(a, int) or a <= 0 for a in asset_ids):
            raise RequestError("asset_ids musí byť zoznam kladných ID")
        limit = self._limit(request.get("limit", 5))
        with self._lock:
            labels, parents = self._labels[kind], self._parents[kind]
        index = self._index_snapshot(use_cuda=True)
        results = {}
        totals: dict[int, float] = {}; counts: dict[int, int] = {}
        for aid in asset_ids:
            if aid not in index[2]:
                continue
            suggestions = self._rank_labels(aid, kind, 1000, index, labels, parents)
            results[str(aid)] = suggestions[:limit]
            for item in suggestions:
                label_id = item["label_id"]
                totals[label_id] = totals.get(label_id, 0.0) + item["score"]
                counts[label_id] = counts.get(label_id, 0) + 1
        denominator = max(1, len(asset_ids))
        combined = [{"label_id": label_id, "score": score / denominator, "count": counts[label_id]}
                    for label_id, score in totals.items()]
        combined.sort(key=lambda row: (-row["score"], -row["count"], row["label_id"]))
        return {"results": results, "combined": combined, "searched": len(index[0])}

    def _content_keywords(self, request: dict[str, Any]) -> dict[str, Any]:
        asset_id = request.get("asset_id")
        if type(asset_id) is not int or asset_id <= 0:
            raise RequestError("Fotka ešte nie je zaindexovaná")
        _, images, positions = self._index_snapshot()
        if asset_id not in positions:
            raise RequestError("Fotka ešte nie je zaindexovaná")
        photo = images[positions[asset_id]].astype(np.float32)
        with self._keyword_lock:
            if self._keyword_matrix is None:
                self._keyword_matrix = self._load_keyword_matrix()
            matrix = self._keyword_matrix
        scores = matrix @ photo
        # Raw cosine similarity is not a probability. Suppress weak matches and distant runners-up.
        cutoff = max(0.12, float(scores.max()) - 0.065)
        return {"words": [KEYWORD_VOCABULARY[i] for i in np.argsort(-scores, kind="stable")[:20]
                          if scores[i] >= cutoff]}

    def _load_keyword_matrix(self) -> np.ndarray:
        prompts = [f"a photo of {word}" for word in KEYWORD_VOCABULARY]
        with self.db_lock:
            placeholders = ",".join("?" for _ in prompts)
            vectors = {text: blob for text, blob in self.db.execute(
                f"SELECT text,vector FROM text_vectors WHERE model=? AND text IN ({placeholders})",
                (self.model_name, *prompts),
            ) if len(blob) == DIMENSION * 2}
        missing = [text for text in prompts if text not in vectors]
        if missing:
            self._start_model()
            embedder = self._wait_model()
            for start in range(0, len(missing), 16):
                batch = missing[start:start + 16]
                values = embedder._encode_texts(batch).cpu().numpy()
                vectors.update((text, value.tobytes()) for text, value in zip(batch, values, strict=True))
            with self.db_lock:
                self.db.executemany("INSERT OR REPLACE INTO text_vectors(model,text,vector) VALUES(?,?,?)",
                                    [(self.model_name, text, vectors[text]) for text in missing])
                self.db.commit()
        return np.stack([np.frombuffer(vectors[text], dtype=np.float16) for text in prompts]).astype(np.float32)

    def _suggest_batch(self, request: dict[str, Any]) -> dict[str, Any]:
        kind = self._kind(request.get("kind"))
        asset_ids = request.get("asset_ids")
        minimum = request.get("min_score", AUTO_GROUP_SCORE)
        if isinstance(minimum, bool) or not isinstance(minimum, (int, float)) or not 0 <= minimum <= 1:
            raise RequestError("min_score musí byť číslo od 0 do 1")
        with self._lock:
            labels, parents = self._labels[kind], self._parents[kind]
        index = self._index_snapshot(use_cuda=True)
        indexed = set(index[0])
        if asset_ids == "unlabeled":
            targets = sorted(indexed - set(labels))
        elif isinstance(asset_ids, list) and all(isinstance(a, int) and not isinstance(a, bool) and a > 0 for a in asset_ids):
            targets = list(dict.fromkeys(asset_ids))
        else:
            raise RequestError('asset_ids musí byť zoznam ID alebo "unlabeled"')
        groups: dict[int, list[tuple[int, float]]] = {}
        candidates = [aid for aid, values in labels.items() if values and aid in index[2]]
        ids, matrix, positions = index
        targets = [aid for aid in targets if aid in indexed]
        if candidates:
            if isinstance(matrix, np.ndarray):
                candidate_matrix = matrix[[positions[aid] for aid in candidates]].astype(np.float32)
                for start in range(0, len(targets), SUGGEST_BATCH_CHUNK):
                    block = targets[start:start + SUGGEST_BATCH_CHUNK]
                    scores = (matrix[[positions[aid] for aid in block]].astype(np.float32) @ candidate_matrix.T).astype(np.float16).astype(np.float32)
                    self._collect_batch_groups(block, candidates, scores, labels, parents, float(minimum), groups)
            else:
                torch = self._cuda_torch
                candidate_pos = torch.as_tensor([positions[a] for a in candidates], device=matrix.device)
                for start in range(0, len(targets), SUGGEST_BATCH_CHUNK):
                    block = targets[start:start + SUGGEST_BATCH_CHUNK]
                    query_pos = torch.as_tensor([positions[a] for a in block], device=matrix.device)
                    with torch.inference_mode():
                        scores = (matrix[query_pos] @ matrix[candidate_pos].T).float().cpu().numpy()
                    self._collect_batch_groups(block, candidates, scores, labels, parents, float(minimum), groups)
        rows = [{"label_id": label_id, "count": len(items),
                 "asset_ids": [aid for aid, _ in sorted(items, key=lambda pair: (-pair[1], pair[0]))]}
                for label_id, items in groups.items()]
        rows.sort(key=lambda row: (-row["count"], row["label_id"]))
        return {"groups": rows}

    @staticmethod
    def _normalise_rows(values: np.ndarray) -> np.ndarray:
        values = np.asarray(values, dtype=np.float32)
        norms = np.linalg.norm(values, axis=1, keepdims=True)
        return values / np.maximum(norms, 1e-12)

    def _cluster_names(self) -> dict[str, np.ndarray] | None:
        try:
            with self.db_lock:
                vectors = {text: np.frombuffer(blob, dtype=np.float16).astype(np.float32) for text, blob in
                           self.db.execute("SELECT text,vector FROM text_vectors WHERE model=?", (self.model_name,))}
            embedder = self._embedder
            get_text_vector = getattr(embedder, "_text_vector", None)
            for word in CLUSTER_VOCABULARY:
                text = f"a photo of {word}"
                if text in vectors or get_text_vector is None: continue
                value = get_text_vector(text)
                if hasattr(value, "float"):
                    value = value.float().cpu().numpy()
                value = np.asarray(value, dtype=np.float16).reshape(-1)
                with self.db_lock:
                    self.db.execute("INSERT OR REPLACE INTO text_vectors(model,text,vector) VALUES(?,?,?)", (self.model_name, text, value.tobytes()))
                    self.db.commit()
                vectors[text] = value.astype(np.float32)
            if any(f"a photo of {word}" not in vectors for word in CLUSTER_VOCABULARY):
                return None
            return {word: vectors[f"a photo of {word}"] for word in CLUSTER_VOCABULARY}
        except Exception:
            return None

    @staticmethod
    def _map_tsne(centroids: np.ndarray) -> np.ndarray:
        values = AIService._normalise_rows(np.asarray(centroids, dtype=np.float64))
        n = len(values)
        if n < 4:
            angles = np.arange(n, dtype=np.float64) * (2 * np.pi / max(1, n))
            return np.column_stack((np.cos(angles), np.sin(angles)))
        distances = np.maximum(0.0, 1.0 - values @ values.T)
        perplexity = min(30.0, (n - 1) / 3.0)
        probabilities = np.zeros((n, n), dtype=np.float64)
        target = np.log(perplexity)
        for i in range(n):
            mask = np.arange(n) != i
            row = distances[i, mask]
            lo, hi, beta = -np.inf, np.inf, 1.0
            for _ in range(60):
                weights = np.exp(-np.maximum(0.0, row - float(row.min())) * min(beta, 1e6))
                total = max(float(weights.sum()), 1e-300)
                entropy = np.log(total) + beta * float((row * weights).sum()) / total
                if abs(entropy - target) < 1e-5: break
                if entropy > target: lo = beta; beta = min(1e6, beta * 2 if np.isinf(hi) else (beta + hi) / 2)
                else: hi = beta; beta = beta / 2 if np.isinf(lo) else (beta + lo) / 2
            probabilities[i, mask] = weights / max(float(weights.sum()), 1e-300)
        p = (probabilities + probabilities.T) / (2 * n)
        rng = np.random.default_rng(MAP_TSNE_SEED)
        centered = values - values.mean(axis=0)
        _, _, vt = np.linalg.svd(centered, full_matrices=False)
        y = centered @ vt[:2].T
        y *= 1e-4 / max(float(np.std(y)), 1e-12)
        velocity = np.zeros_like(y); gains = np.ones_like(y)
        for iteration in range(MAP_TSNE_ITERATIONS):
            delta = y[:, None, :] - y[None, :, :]
            inv = 1 / (1 + np.sum(delta * delta, axis=2)); np.fill_diagonal(inv, 0)
            q = inv / max(float(inv.sum()), 1e-300)
            exaggeration = 4.0 if iteration < MAP_TSNE_EXAGGERATION else 1.0
            grad = 4 * np.sum(((exaggeration * p - q) * inv)[:, :, None] * delta, axis=1)
            gains = np.where((grad > 0) != (velocity > 0), gains + .2, gains * .8).clip(.01, 100)
            velocity = .8 * velocity - 200 * gains * grad
            y += velocity; y -= y.mean(axis=0)
        lo, hi = y.min(axis=0), y.max(axis=0)
        extent = max(float((hi - lo).max()), 1e-12)
        return (y - (lo + hi) / 2) / extent + .5

    @staticmethod
    def _map_pack(coords: np.ndarray, counts: np.ndarray) -> tuple[np.ndarray, np.ndarray]:
        """Turn t-SNE centroid positions into touching, non-overlapping islands.

        Island area is proportional to its photo count. t-SNE alone leaves tight clumps
        and large voids, so the circles are pushed apart where they overlap and pulled
        gently back towards their t-SNE position; the result is scaled into the unit square.
        """
        k = len(counts)
        rad = np.sqrt(np.maximum(counts, 1).astype(np.float64))
        if k == 1:
            return np.array([[0.5, 0.5]]), np.array([0.47])
        centred = coords.astype(np.float64) - coords.mean(axis=0)
        extent = max(float(np.sqrt((centred ** 2).sum(axis=1)).max()), 1e-9)
        target = np.sqrt(float((rad ** 2).sum()) / MAP_PACK_DENSITY)
        anchor = centred * (target / extent)
        pos = anchor.copy()
        need = (rad[:, None] + rad[None, :]) * MAP_PACK_GAP
        for step in range(MAP_PACK_ITERATIONS):
            delta = pos[:, None, :] - pos[None, :, :]
            dist = np.sqrt((delta ** 2).sum(axis=2))
            np.fill_diagonal(dist, np.inf)
            overlap = np.clip(need - dist, 0.0, None)
            np.fill_diagonal(overlap, 0.0)
            safe = np.where(dist > 1e-9, dist, 1.0)
            direction = delta / safe[:, :, None]
            # coincident centres get a deterministic direction
            zero = dist <= 1e-9
            if zero.any():
                ii, jj = np.nonzero(zero)
                angle = (ii * 0.61803398875 + jj * 0.38196601125) * 2 * np.pi
                direction[ii, jj, 0] = np.cos(angle) * np.sign(ii - jj)
                direction[ii, jj, 1] = np.sin(angle) * np.sign(ii - jj)
            pos += (direction * (overlap * 0.5)[:, :, None]).sum(axis=1) * 0.6
            pull = 0.03 * (1.0 - step / MAP_PACK_ITERATIONS)
            pos += (anchor - pos) * pull
        low = (pos - rad[:, None]).min(axis=0)
        high = (pos + rad[:, None]).max(axis=0)
        span = max(float((high - low).max()), 1e-9)
        offset = (span - (high - low)) / 2.0
        return (pos - low + offset) / span, rad / span * MAP_RADIUS_SCALE

    def _map_build(self) -> dict[str, Any]:
        started = time.monotonic()
        ids, matrix, positions = self._index_snapshot(use_cuda=True)
        ids = sorted(ids)
        n = len(ids)
        if not n:
            with self.db_lock:
                self.db.execute("BEGIN IMMEDIATE")
                self.db.execute("DELETE FROM map_points"); self.db.execute("DELETE FROM map_clusters")
                self.db.execute("INSERT OR REPLACE INTO meta(key,value) VALUES('map_built_at',?)", (str(time.time()),))
                self.db.execute("INSERT OR REPLACE INTO meta(key,value) VALUES('map_photo_count','0')")
                self.db.commit()
            return {"points": 0, "clusters": 0, "seconds": time.monotonic()-started}
        k = max(8, min(600, int(round(1.5 * np.sqrt(n)))))
        if n < k: k = n
        if isinstance(matrix, np.ndarray):
            ordered = [positions[aid] for aid in ids]
            vectors = self._normalise_rows(matrix if ordered == list(range(n)) else matrix[ordered])
            centers = vectors[np.linspace(0, n-1, k, dtype=np.int64)].copy()
            for _ in range(MAP_ITERATIONS):
                assignment = np.argmax(vectors @ centers.T, axis=1)
                for cid in range(k):
                    members = vectors[assignment == cid]
                    if len(members): centers[cid] = self._normalise_rows(members.mean(axis=0, keepdims=True))[0]
            sims = vectors @ centers.T; assignment = np.argmax(sims, axis=1)
        else:
            torch = self._cuda_torch
            pos = torch.as_tensor([positions[aid] for aid in ids], device=matrix.device)
            with torch.inference_mode():
                vectors_t = matrix[pos].float(); vectors_t /= vectors_t.norm(dim=1, keepdim=True).clamp_min(1e-12)
                centers_t = vectors_t[torch.as_tensor(np.linspace(0,n-1,k,dtype=np.int64),device=matrix.device)].clone()
                for _ in range(MAP_ITERATIONS):
                    parts = [ (vectors_t[s:s+8192] @ centers_t.T).argmax(dim=1) for s in range(0,n,8192) ]
                    assignment_t = torch.cat(parts); counts_t=torch.bincount(assignment_t,minlength=k); sums=torch.zeros_like(centers_t); sums.index_add_(0,assignment_t,vectors_t)
                    means=sums/counts_t.clamp_min(1).float().unsqueeze(1); means=means/means.norm(dim=1,keepdim=True).clamp_min(1e-12); centers_t=torch.where((counts_t>0).unsqueeze(1),means,centers_t)
                centers=centers_t.cpu().numpy(); vectors=vectors_t.cpu().numpy(); sims=vectors @ centers.T; assignment=np.argmax(sims,axis=1)
        nonempty = [cid for cid in range(k) if np.any(assignment == cid)]
        centers = self._normalise_rows(centers[nonempty]); remap={old:i for i,old in enumerate(nonempty)}
        assignment=np.array([remap[int(a)] for a in assignment],dtype=np.int32); k=len(nonempty)
        coords=self._map_tsne(centers)
        counts=np.bincount(assignment,minlength=k)
        coords,radii=self._map_pack(coords,counts)
        names=self._cluster_names(); vocab=list(CLUSTER_VOCABULARY); name_matrix=None
        if names:
            try: name_matrix=self._normalise_rows(np.stack([names[word] for word in vocab]))
            except Exception: name_matrix=None
        points=[]; cluster_rows=[]
        for cid in range(k):
            member=np.flatnonzero(assignment==cid); order=member[np.argsort(-sims[member,cid],kind='stable')]
            for rank,idx in enumerate(order):
                radius=radii[cid]*np.sqrt((rank+.5)/len(order)); angle=rank*MAP_GOLDEN_ANGLE
                points.append((ids[int(idx)],float(np.clip(coords[cid,0]+radius*np.cos(angle),0,1)),float(np.clip(coords[cid,1]+radius*np.sin(angle),0,1)),cid))
            name=None
            if name_matrix is not None:
                name=vocab[int(np.argmax(name_matrix @ centers[cid]))].capitalize()
            cluster_rows.append((cid,float(coords[cid,0]),float(coords[cid,1]),float(radii[cid]),int(counts[cid]),name))
        built_at=time.time(); elapsed=time.monotonic()-started
        with self.db_lock:
            self.db.execute("BEGIN IMMEDIATE")
            self.db.execute("DELETE FROM map_points");self.db.execute("DELETE FROM map_clusters")
            self.db.executemany("INSERT INTO map_points(asset_id,x,y,cluster) VALUES(?,?,?,?)",points)
            self.db.executemany("INSERT INTO map_clusters(id,x,y,r,count,name) VALUES(?,?,?,?,?,?)",cluster_rows)
            self.db.execute("INSERT OR REPLACE INTO meta(key,value) VALUES('map_built_at',?)",(str(built_at),))
            self.db.execute("INSERT OR REPLACE INTO meta(key,value) VALUES('map_photo_count',?)",(str(n),))
            self.db.commit()
        return {"points":n,"clusters":k,"seconds":elapsed}

    def _clusters(self, request: dict[str, Any]) -> dict[str, Any]:
        min_size = request.get("min_size", CLUSTER_MIN_SIZE)
        min_sim = request.get("min_sim", CLUSTER_MIN_SIM)
        limit = request.get("limit", 30)
        if isinstance(min_size, bool) or not isinstance(min_size, int) or not 1 <= min_size <= 100000:
            raise RequestError("min_size musí byť kladné celé číslo")
        if isinstance(min_sim, bool) or not isinstance(min_sim, (int, float)) or not np.isfinite(min_sim) or not -1 <= min_sim <= 1:
            raise RequestError("min_sim musí byť číslo od -1 do 1")
        if isinstance(limit, bool) or not isinstance(limit, int) or not 1 <= limit <= 100:
            raise RequestError("limit musí byť celé číslo od 1 do 100")
        ids, matrix, positions = self._index_snapshot(use_cuda=True)
        with self._lock:
            labels = self._labels["category"]
            targets = sorted(aid for aid in ids if not labels.get(aid))
            category_labels = {aid: set(values) for aid, values in labels.items() if values and aid in positions}
            parents = dict(self._parents["category"])
        unlabeled = len(targets)
        if unlabeled < min_size:
            return {"groups": [], "unlabeled": unlabeled, "clustered": 0}
        n = len(targets)
        k = max(2, min(256, int(round(np.sqrt(n / 2.0)))))
        core_clusters = []
        if isinstance(matrix, np.ndarray):
            target_positions = [positions[aid] for aid in targets]
            vectors = self._normalise_rows(matrix if target_positions == list(range(len(ids))) else matrix[target_positions])
            centroids = vectors[np.linspace(0, n - 1, k, dtype=np.int64)].copy()
            for _ in range(CLUSTER_ITERATIONS):
                assignments = np.argmax(vectors @ centroids.T, axis=1)
                for cluster_id in range(k):
                    members = vectors[assignments == cluster_id]
                    if len(members):
                        centroids[cluster_id] = self._normalise_rows(members.mean(axis=0, keepdims=True))[0]
            similarities = vectors @ centroids.T
            assignments = np.argmax(similarities, axis=1)
            assigned_scores = similarities[np.arange(n), assignments]
            for cluster_id in range(k):
                member_idx = np.flatnonzero((assignments == cluster_id) & (assigned_scores >= min_sim))
                if len(member_idx) < min_size:
                    continue
                centroid = self._normalise_rows(vectors[member_idx].mean(axis=0, keepdims=True))[0]
                core_sim = vectors[member_idx] @ centroid
                keep = core_sim >= min_sim
                member_idx, core_sim = member_idx[keep], core_sim[keep]
                if len(member_idx) >= min_size:
                    core_clusters.append((centroid, member_idx, core_sim))
        else:
            torch = self._cuda_torch
            target_positions = torch.as_tensor([positions[aid] for aid in targets], device=matrix.device)
            with torch.inference_mode():
                tensor = matrix[target_positions].float()
                tensor = tensor / tensor.norm(dim=1, keepdim=True).clamp_min(1e-12)
                centers = tensor[torch.as_tensor(np.linspace(0, n - 1, k, dtype=np.int64), device=matrix.device)].clone()
                for _ in range(CLUSTER_ITERATIONS):
                    assign_parts = [(tensor[start:start + 8192] @ centers.T).argmax(dim=1) for start in range(0, n, 8192)]
                    assignments_t = torch.cat(assign_parts)
                    counts = torch.bincount(assignments_t, minlength=k)
                    sums = torch.zeros_like(centers)
                    sums.index_add_(0, assignments_t, tensor)
                    means = sums / counts.clamp_min(1).to(dtype=torch.float32).unsqueeze(1)
                    means = means / means.norm(dim=1, keepdim=True).clamp_min(1e-12)
                    centers = torch.where((counts > 0).unsqueeze(1), means, centers)
                score_parts, assign_parts = [], []
                for start in range(0, n, 8192):
                    scores = tensor[start:start + 8192] @ centers.T
                    values, chosen = scores.max(dim=1)
                    score_parts.append(values)
                    assign_parts.append(chosen)
                assignments_t = torch.cat(assign_parts)
                assigned_scores_t = torch.cat(score_parts)
                assignments = assignments_t.cpu().numpy()
                for cluster_id in range(k):
                    member_idx = torch.nonzero((assignments_t == cluster_id) & (assigned_scores_t >= min_sim), as_tuple=False).flatten()
                    if member_idx.numel() < min_size:
                        continue
                    centroid = tensor[member_idx].mean(dim=0)
                    centroid = centroid / centroid.norm().clamp_min(1e-12)
                    core_sim = tensor[member_idx] @ centroid
                    keep = core_sim >= min_sim
                    member_idx, core_sim = member_idx[keep], core_sim[keep]
                    if member_idx.numel() >= min_size:
                        core_clusters.append((centroid.cpu().numpy(), member_idx.cpu().numpy(), core_sim.cpu().numpy()))
                vectors = tensor.cpu().numpy()
        if not core_clusters:
            return {"groups": [], "unlabeled": unlabeled, "clustered": 0}
        with self.db_lock:
            ignored_rows = self.db.execute("SELECT vector FROM cluster_ignored ORDER BY id").fetchall()
        ignored = [np.frombuffer(blob, dtype=np.float16).astype(np.float32) for (blob,) in ignored_rows]
        if ignored:
            ignored_matrix = self._normalise_rows(np.stack(ignored))
            core_clusters = [item for item in core_clusters if float((ignored_matrix @ item[0]).max()) < CLUSTER_IGNORE_SIM]
        core_clusters.sort(key=lambda item: (-len(item[1]), min(targets[int(i)] for i in item[1])))
        name_vectors = self._cluster_names()
        candidate_ids = sorted(category_labels)
        candidate_matrix = (matrix[[positions[aid] for aid in candidate_ids]].astype(np.float32) if isinstance(matrix, np.ndarray) and candidate_ids
                            else None)
        if not isinstance(matrix, np.ndarray) and candidate_ids:
            torch = self._cuda_torch
            candidate_pos = torch.as_tensor([positions[aid] for aid in candidate_ids], device=matrix.device)
            with torch.inference_mode():
                candidate_matrix = matrix[candidate_pos].float().cpu().numpy()
        groups, clustered = [], 0
        for centroid, member_idx, _ in core_clusters[:limit]:
            sims = vectors[member_idx] @ centroid
            order = np.argsort(-sims, kind="stable")
            ordered_idx = member_idx[order]
            ordered_sims = sims[order]
            member_ids = [targets[int(i)] for i in ordered_idx]
            category_votes: dict[int, int] = {}
            if candidate_ids and candidate_matrix is not None:
                neighbor_similarities = vectors[ordered_idx[:40]] @ candidate_matrix.T
                for row, target_id in enumerate(member_ids[:40]):
                    scores = neighbor_similarities[row].copy()
                    if target_id in candidate_ids:
                        scores[candidate_ids.index(target_id)] = -np.inf
                    take = min(SUGGEST_K, len(candidate_ids))
                    chosen = np.argpartition(-scores, take - 1)[:take] if take else np.empty(0, dtype=np.int64)
                    chosen = chosen[np.argsort(-scores[chosen], kind="stable")]
                    neighbors = [(candidate_ids[int(i)], float(scores[i])) for i in chosen if np.isfinite(scores[i])]
                    ranked = self._rank_neighbors(target_id, neighbors, 1, category_labels, parents)
                    if ranked:
                        category_votes[ranked[0]["label_id"]] = category_votes.get(ranked[0]["label_id"], 0) + 1
            label_id, label_score = (None, None)
            if category_votes:
                winner, votes = min(category_votes.items(), key=lambda item: (-item[1], item[0]))
                if votes >= min(20, len(member_ids[:40]) / 2):
                    label_id, label_score = winner, votes / max(1, len(member_ids[:40]))
            name = None
            if name_vectors:
                name = max(name_vectors, key=lambda word: float(np.dot(name_vectors[word], centroid))).capitalize()
            total = len(member_ids)
            clustered += total
            groups.append({"ids": member_ids[:500], "total": total, "cohesion": float(ordered_sims.mean()),
                           "label_id": label_id, "label_score": label_score, "name": name})
        return {"groups": groups, "unlabeled": unlabeled, "clustered": clustered}

    def _cluster_ignore(self, request: dict[str, Any]) -> dict[str, Any]:
        asset_ids = request.get("asset_ids")
        if not isinstance(asset_ids, list) or not asset_ids or any(isinstance(a, bool) or not isinstance(a, int) or a <= 0 for a in asset_ids):
            raise RequestError("asset_ids musí byť zoznam kladných ID")
        _, matrix, positions = self._index_snapshot(use_cuda=True)
        vectors = matrix[[positions[aid] for aid in dict.fromkeys(asset_ids) if aid in positions]] if isinstance(matrix, np.ndarray) else None
        if not isinstance(matrix, np.ndarray):
            torch = self._cuda_torch
            valid = [aid for aid in dict.fromkeys(asset_ids) if aid in positions]
            if valid:
                pos = torch.as_tensor([positions[aid] for aid in valid], device=matrix.device)
                with torch.inference_mode():
                    vectors = matrix[pos].float().cpu().numpy()
            else:
                vectors = []
        if len(vectors) == 0:
            return {"ignored": 0}
        mean = self._normalise_rows(np.asarray(vectors, dtype=np.float32).mean(axis=0, keepdims=True))[0]
        with self.db_lock:
            self.db.execute("INSERT INTO cluster_ignored(vector,created) VALUES(?,?)", (mean.astype(np.float16).tobytes(), time.time()))
            self.db.commit()
            count = int(self.db.execute("SELECT count(*) FROM cluster_ignored").fetchone()[0])
            return {"ignored": count}

    def _cluster_ignore_reset(self, request: dict[str, Any]) -> dict[str, Any]:
        with self.db_lock:
            self.db.execute("DELETE FROM cluster_ignored")
            self.db.commit()
        return {"ignored": 0}

    def _similarity_scores(self, index: tuple[list[int], Any, dict[int, int]], rows: list[int], cols: list[int]) -> np.ndarray:
        ids, matrix, positions = index
        if isinstance(matrix, np.ndarray):
            return (matrix[rows].astype(np.float32) @ matrix[cols].astype(np.float32).T).astype(np.float16)
        torch = self._cuda_torch
        ri = torch.as_tensor(rows, device=matrix.device)
        ci = torch.as_tensor(cols, device=matrix.device)
        with torch.inference_mode():
            return (matrix[ri] @ matrix[ci].T).float().cpu().numpy()

    @staticmethod
    def _round_duplicate_scores(matrix: np.ndarray, rows: np.ndarray, cols: np.ndarray, scores: np.ndarray) -> np.ndarray:
        values = scores.astype(np.float16)
        rounded = values.astype(np.float32)
        direction = np.where(scores >= rounded, np.float16(np.inf), np.float16(-np.inf))
        midpoint = (rounded + np.nextafter(values, direction).astype(np.float32)) * 0.5
        # Float32 CPU and CUDA dot products can straddle the same float16 midpoint.
        # Resolve only those scores with the same higher-precision dot product.
        borderline = np.flatnonzero((np.abs(scores - midpoint) <= 2e-6) & (rows != cols))
        for start in range(0, len(borderline), 256):
            chunk = borderline[start:start + 256]
            exact = np.einsum("ij,ij->i", matrix[rows[chunk]], matrix[cols[chunk]], dtype=np.float64)
            values[chunk] = exact.astype(np.float16)
        return values

    def _duplicate_groups(self, request: dict[str, Any]) -> dict[str, Any]:
        threshold = request.get("threshold")
        if isinstance(threshold, bool) or not isinstance(threshold, (int, float)) or not -1 <= threshold <= 1:
            raise RequestError("threshold musí byť číslo od -1 do 1")
        requested = request.get("asset_ids")
        if requested is not None and (not isinstance(requested, list) or any(isinstance(a, bool) or not isinstance(a, int) for a in requested)):
            raise RequestError("asset_ids musí byť zoznam ID")
        with self.db_lock:
            ids, matrix, positions = self._index_snapshot(use_cuda=True)
            host = self._vectors.snapshot()[1]
        selected = list(dict.fromkeys(requested if requested is not None else ids))
        selected = [aid for aid in selected if aid in positions]
        positions_in_matrix = [positions[aid] for aid in selected]
        neighbors: list[list[tuple[int, float]]] = [[] for _ in selected]
        block_size = 2048
        # Only pairs above the threshold leave the GPU; a per-element Python loop over the
        # full score rows took ~9 s for 8k photos and would grow quadratically.
        if isinstance(matrix, np.ndarray):
            sub = matrix if selected == ids else matrix[positions_in_matrix]
            all_rows, all_cols, all_scores = [], [], []
            for start in range(0, len(selected), block_size):
                left = sub[start:start + block_size].astype(np.float32)
                for col_start in range(0, len(selected), 8192):
                    right = sub[col_start:col_start + 8192].astype(np.float32)
                    score = left @ right.T
                    rows, cols = np.nonzero(score >= threshold - 0.001)
                    floats = score[rows, cols]
                    rows, cols = rows + start, cols + col_start
                    values = self._round_duplicate_scores(sub, rows, cols, floats)
                    keep = (rows != cols) & (values >= threshold)
                    all_rows.append(rows[keep]); all_cols.append(cols[keep]); all_scores.append(values[keep])
            rows = np.concatenate(all_rows) if all_rows else np.empty(0, dtype=np.int64)
            cols = np.concatenate(all_cols) if all_cols else np.empty(0, dtype=np.int64)
            scores = np.concatenate(all_scores) if all_scores else np.empty(0, dtype=np.float16)
            if len(rows):
                row_order = np.argsort(rows, kind="stable")
                rows, cols, scores = rows[row_order], cols[row_order], scores[row_order]
            counts = np.bincount(rows, minlength=len(selected))
            offsets = np.concatenate(([0], np.cumsum(counts)))
            order = sorted(range(len(selected)), key=lambda i: (-int(counts[i]), selected[i]))
            assigned = np.zeros(len(selected), dtype=bool)
            groups = []
            for seed in order:
                if assigned[seed]: continue
                start, end = int(offsets[seed]), int(offsets[seed + 1])
                members = cols[start:end]
                keep = ~assigned[members]
                members = members[keep]
                if not len(members): continue
                sims = scores[start:end][keep]
                assigned[seed] = True
                assigned[members] = True
                groups.append({"asset_ids": [selected[seed], *(selected[int(other)] for other in members)],
                               "min_sim": float(sims.min()), "max_sim": float(sims.max())})
            groups.sort(key=lambda g: (-len(g["asset_ids"]), -g["max_sim"], g["asset_ids"][0]))
            return {"groups": groups}
        elif selected:
            torch = self._cuda_torch
            with torch.inference_mode():
                sub = matrix if selected == ids else matrix[torch.as_tensor(positions_in_matrix, device=matrix.device)]
                host = host if selected == ids else host[positions_in_matrix]
                # Use float32 accumulation like NumPy, then round to float16 for grouping.
                sub = sub.float()
                for start in range(0, len(selected), block_size):
                    score = sub[start:start + block_size] @ sub.T
                    hits = (score >= threshold - 0.001).nonzero()
                    floats = score[hits[:, 0], hits[:, 1]].cpu().numpy()
                    rows, cols = hits.cpu().numpy().T
                    values = self._round_duplicate_scores(host, start + rows, cols, floats)
                    keep = (start + rows != cols) & (values >= threshold)
                    for r, c, value in zip(rows[keep].tolist(), cols[keep].tolist(), values[keep].tolist()):
                        neighbors[start + r].append((c, value))
        order = sorted(range(len(selected)), key=lambda i: (-len(neighbors[i]), selected[i]))
        assigned: set[int] = set()
        groups = []
        for seed in order:
            if seed in assigned: continue
            members = [(other, sim) for other, sim in neighbors[seed] if other not in assigned]
            if not members: continue
            assigned.add(seed)
            assigned.update(other for other, _ in members)
            sims = [sim for _, sim in members]
            groups.append({"asset_ids": [selected[seed], *(selected[other] for other, _ in members)], "min_sim": min(sims), "max_sim": max(sims)})
        groups.sort(key=lambda g: (-len(g["asset_ids"]), -g["max_sim"], g["asset_ids"][0]))
        return {"groups": groups}

    @staticmethod
    def _catalog_day(value):
        if isinstance(value, str) and len(value) >= 10:
            day = value[:10]
            try:
                if day[4] == day[7] == "-":
                    return date.fromisoformat(day).isoformat()
            except ValueError:
                pass
        return None

    def _incoming_folder_metadata(self, catalog, incoming, extensions):
        # SQLite's read-only connection observes catalogue edits, including writes from Tauri.
        with self._folder_catalog_lock:
            if catalog != self._folder_catalog_path:
                connection = sqlite3.connect(Path(catalog).absolute().as_uri() + "?mode=ro",
                                             uri=True, check_same_thread=False)
                if self._folder_catalog is not None:
                    self._folder_catalog.close()
                self._folder_catalog, self._folder_catalog_path = connection, catalog
                self._folder_metadata_key = None
            connection = self._folder_catalog
            version = connection.execute("PRAGMA data_version").fetchone()[0]
            prefix = incoming.replace("/", "\\").rstrip("\\").lower() + "\\"
            key = (version, prefix, tuple(extensions))
            if key != self._folder_metadata_key:
                metadata, folders = {}, {}
                if extensions:
                    marks = ",".join("?" for _ in extensions)
                    rows = connection.execute(
                        f"SELECT id,path,exif_date,file_modified FROM assets WHERE is_listed=1 AND path IS NOT NULL AND ext_lower IN ({marks}) ORDER BY id",
                        extensions,
                    )
                    for aid, path, capture, modified in rows:
                        canonical = path.replace("/", "\\")
                        if canonical.lower().startswith(prefix):
                            metadata[aid] = (None, self._catalog_day(capture))
                        else:
                            folder = ntpath.dirname(canonical)
                            if folder:
                                folder = folders.setdefault(folder.rstrip("\\").lower(), folder)
                                metadata[aid] = (folder, self._catalog_day(capture) or self._catalog_day(modified))
                self._folder_metadata, self._folder_metadata_key = metadata, key
            return self._folder_metadata

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

    def _keyword_matches(self, request: dict[str, Any]) -> dict[str, Any]:
        ids, matrix, positions = self._index_snapshot()
        seeds = [positions[aid] for aid in request["seeds"] if aid in positions]
        if not seeds:
            return {"ids": [], "examples": 0}
        # Spread the sample over the tagged photos so a long burst does not fill all slots.
        if len(seeds) > 32:
            seeds = [seeds[i] for i in np.linspace(0, len(seeds) - 1, 32, dtype=int)]
        examples = matrix[seeds].astype(np.float32)
        excluded = set(request["excluded"]).union(request["seeds"])
        matches = []
        for start in range(0, len(ids), 2048):
            scores = (matrix[start:start + 2048].astype(np.float32) @ examples.T).max(axis=1)
            matches.extend((float(score), aid) for score, aid in zip(scores, ids[start:start + 2048])
                           if score >= 0.82 and aid not in excluded)
        matches.sort(key=lambda row: (-row[0], row[1]))
        return {"ids": [aid for _, aid in matches[:500]], "examples": len(seeds)}

    def _incoming_groups(self, request: dict[str, Any]) -> dict[str, Any]:
        metadata = self._incoming_folder_metadata(request["catalog"], request["incoming"], request["extensions"])
        _, matrix, positions = self._index_snapshot()
        photos = sorted(aid for aid, (folder, _) in metadata.items() if folder is None)
        indexed = [aid for aid in photos if aid in positions]
        limit = 2000
        ids = indexed[-limit:]
        vectors = matrix[[positions[aid] for aid in ids]].astype(np.float32)
        similarities = vectors @ vectors.T
        groups: list[list[int]] = []
        # Every photo must resemble its group's first photo, avoiding chains of increasingly unrelated shots.
        remaining = np.arange(len(ids))
        while remaining.size:
            scores = similarities[remaining[0], remaining]
            members = scores >= 0.88
            members[0] = True
            groups.append([ids[i] for i in remaining[members]])
            remaining = remaining[~members]
        groups.sort(key=lambda group: (-len(group), group[0]))
        return {"groups": groups, "unindexed": len(photos) - len(indexed), "omitted": len(indexed) - len(ids)}

    def _suggest_folders(self, request: dict[str, Any]) -> dict[str, Any]:
        selected = request.get("asset_ids")
        if not isinstance(selected, list):
            raise RequestError("asset_ids musí byť zoznam")
        started = time.perf_counter()
        catalog = request.get("catalog")
        if catalog:
            metadata = self._incoming_folder_metadata(catalog, request["incoming"], request["extensions"])
            # Refresh only the EXIF lookups asked for by this request.
            overrides = {int(aid): day for aid, day in request.get("days", {}).items()}
        else:
            photos = request.get("photos")
            if not isinstance(photos, list):
                raise RequestError("photos musí byť zoznam")
            metadata = {aid: (folder, day) for aid, folder, day in photos}
            overrides = {}
        metadata_s = time.perf_counter() - started
        started = time.perf_counter()
        index = self._folder_vector_snapshot() if catalog else self._index_snapshot(use_cuda=True)
        _, matrix, positions = index
        vector_s = time.perf_counter() - started
        started = time.perf_counter()
        queries = [aid for aid in dict.fromkeys(selected)
                   if aid in positions and aid in metadata and metadata[aid][0] is None]
        candidates = [(positions[aid], aid, folder, overrides.get(aid, day)) for aid, (folder, day) in metadata.items()
                      if aid in positions and folder is not None]
        candidates.sort(key=lambda row: row[0])
        if not queries or not candidates:
            return {"folders": []}
        indexes, candidate_ids, folders, days = zip(*candidates)
        days = np.asarray(days, dtype=object)
        if isinstance(matrix, np.ndarray):
            vectors = matrix[list(indexes)].astype(np.float32)
        else:
            torch = self._cuda_torch
            vectors = matrix[torch.as_tensor(indexes, device=matrix.device)]
        votes: dict[str, float] = {}
        best_scores = np.full(len(candidates), -np.inf, dtype=np.float32) if request.get("include_examples") else None
        missing_days: set[int] = set()
        resolve_days = request.get("resolve_days") is True
        for start in range(0, len(queries), 32):
            batch = queries[start:start + 32]
            indexes = [positions[aid] for aid in batch]
            if isinstance(matrix, np.ndarray):
                scores = matrix[indexes].astype(np.float32) @ vectors.T
                if catalog:
                    scores = scores.astype(np.float16).astype(np.float32)
            else:
                with torch.inference_mode():
                    scores = (matrix[indexes] @ vectors.T).float().cpu().numpy()
            if best_scores is not None:
                np.maximum(best_scores, scores.max(axis=0), out=best_scores)
            for aid, row in zip(batch, scores):
                day = overrides.get(aid, metadata[aid][1])
                if day:
                    row[days == day] = -np.inf
                positive = np.flatnonzero(row > 0)
                if len(positive) > 25:
                    cutoff = np.partition(row[positive], -25)[-25]
                    stronger = positive[row[positive] > cutoff]
                    tied = positive[row[positive] == cutoff]
                    positive = np.concatenate((stronger, tied[:25 - len(stronger)]))
                for index in positive:
                    if resolve_days and day and days[index] is None:
                        missing_days.add(candidate_ids[index])
                    folder = folders[index]
                    score = float(row[index])
                    votes[folder] = votes.get(folder, 0.0) + score ** 4
        total = sum(votes.values())
        ranked = sorted(votes, key=lambda folder: (-votes[folder], folder))
        result = {"folders": [{"folder": folder, "share": votes[folder] / total} for folder in ranked[:3]]}
        if best_scores is not None:
            examples: dict[str, list[tuple[float, int]]] = {folder: [] for folder in ranked[:3]}
            for index in np.flatnonzero(best_scores >= 0.82):
                if folders[index] in examples:
                    examples[folders[index]].append((-float(best_scores[index]), candidate_ids[index]))
            for entry in result["folders"]:
                entry["exampleIds"] = [aid for _, aid in sorted(examples[entry["folder"]])[:12]]
        if missing_days:
            result["missing_days"] = sorted(missing_days)
        if os.environ.get("DDREAMS_INCOMING_TIMINGS"):
            _stderr(f"incoming folder Python: metadata {metadata_s*1000:.1f} ms, vectors {vector_s*1000:.1f} ms, "
                    f"ranking {(time.perf_counter()-started)*1000:.1f} ms ({len(candidates)} candidates)")
        return result

    def _series(self, request: dict[str, Any]) -> dict[str, Any]:
        items = request.get("items")
        max_gap, min_sim = request.get("max_gap"), request.get("min_sim")
        if not isinstance(items, list):
            raise RequestError("items musí byť zoznam")
        if isinstance(max_gap, bool) or not isinstance(max_gap, (int, float)) or not np.isfinite(max_gap) or max_gap < 0:
            raise RequestError("max_gap musí byť nezáporné číslo")
        if isinstance(min_sim, bool) or not isinstance(min_sim, (int, float)) or not np.isfinite(min_sim) or not -1 <= min_sim <= 1:
            raise RequestError("min_sim musí byť číslo od -1 do 1")
        rows, seen = [], set()
        for index, item in enumerate(items):
            if not isinstance(item, dict):
                raise RequestError("Každá položka musí byť objekt")
            aid, stamp = item.get("asset_id"), item.get("t")
            if isinstance(aid, bool) or not isinstance(aid, int) or aid <= 0 or aid in seen:
                raise RequestError("asset_id musí byť kladné a jedinečné")
            if stamp is not None and (isinstance(stamp, bool) or not isinstance(stamp, (int, float)) or not np.isfinite(stamp)):
                raise RequestError("t musí byť číslo alebo null")
            seen.add(aid)
            rows.append((aid, float(stamp) if stamp is not None else None, index))
        rows.sort(key=lambda row: (row[1] is None, row[1] if row[1] is not None else 0.0, row[2]))
        _, matrix, positions = self._index_snapshot(use_cuda=True)
        pair_positions, pair_ids, pair_indices = [], [], []
        for index in range(1, len(rows)):
            left, right = rows[index - 1], rows[index]
            if left[1] is not None and right[1] is not None and right[1] - left[1] <= max_gap and left[0] in positions and right[0] in positions:
                pair_positions.append((positions[left[0]], positions[right[0]]))
                pair_ids.append((left[0], right[0]))
                pair_indices.append(index)
        similarities = np.empty(len(pair_positions), dtype=np.float32)
        if pair_positions:
            left_pos, right_pos = np.asarray(pair_positions, dtype=np.int64).T
            if isinstance(matrix, np.ndarray):
                left_vectors = matrix[left_pos].astype(np.float32)
                right_vectors = matrix[right_pos].astype(np.float32)
                similarities = np.sum(left_vectors * right_vectors, axis=1, dtype=np.float32)
            else:
                torch = self._cuda_torch
                with torch.inference_mode():
                    left_vectors = matrix[torch.as_tensor(left_pos, device=matrix.device)]
                    right_vectors = matrix[torch.as_tensor(right_pos, device=matrix.device)]
                    similarities = (left_vectors.float() * right_vectors.float()).sum(dim=1).cpu().numpy()
        linked = {index: float(score) >= min_sim for index, score in zip(pair_indices, similarities)}
        by_id = {row[0]: index for index, row in enumerate(rows)}
        sharp_by_id: dict[int, float | None] = {aid: None for aid, _, _ in rows}
        with self.db_lock:
            for start in range(0, len(rows), 500):
                chunk = [row[0] for row in rows[start:start + 500]]
                marks = ",".join("?" for _ in chunk)
                for aid, sharp in self.db.execute(f"SELECT asset_id,sharp FROM quality WHERE asset_id IN ({marks})", chunk):
                    sharp_by_id[int(aid)] = float(sharp) if sharp is not None else None
        faces_by_id = {}
        if request.get("face_aware") is True and self.faces_db is not None:
            with self.faces_lock:
                for start in range(0, len(rows), 500):
                    chunk = [row[0] for row in rows[start:start + 500]]
                    marks = ",".join("?" for _ in chunk)
                    faces_by_id.update((aid, (count, confidence or 0.0)) for aid, count, confidence in
                        self.faces_db.execute(f"SELECT asset_id,count(*),avg(det_score) FROM faces WHERE asset_id IN ({marks}) GROUP BY asset_id", chunk))
        groups, chain = [], []
        def finish_chain() -> None:
            if len(chain) < 2:
                return
            ids = [rows[i][0] for i in chain]
            best = max(ids, key=lambda aid: (*faces_by_id.get(aid, (0, 0.0)),
                                            sharp_by_id[aid] if sharp_by_id[aid] is not None else -1.0,
                                            -rows[by_id[aid]][1]))
            groups.append({"ids": ids, "best": best, "sharp": {str(aid): sharp_by_id[aid] for aid in ids}})
        for index, (aid, stamp, _) in enumerate(rows):
            if stamp is None or aid not in positions:
                finish_chain(); chain = []
                continue
            if index > 0 and linked.get(index, False):
                chain.append(index)
            else:
                finish_chain(); chain = [index]
        finish_chain()
        return {"groups": groups}

    def _order_by_similarity(self, request: dict[str, Any]) -> dict[str, Any]:
        asset_ids = request.get("asset_ids")
        if not isinstance(asset_ids, list) or any(isinstance(a, bool) or not isinstance(a, int) for a in asset_ids):
            raise RequestError("asset_ids musí byť zoznam ID")
        index = self._index_snapshot(use_cuda=True)
        ids, matrix, positions = index
        requested = list(dict.fromkeys(asset_ids))
        indexed = [aid for aid in requested if aid in positions]
        unindexed = [aid for aid in requested if aid not in positions]
        if len(indexed) <= 3000:
            clusters = [indexed]
        else:
            # Deterministic coarse buckets keep the exact nearest-neighbour work bounded.
            centers = indexed[::max(1, len(indexed) // 64)][:64]
            center_pos = [positions[a] for a in centers]
            if isinstance(matrix, np.ndarray):
                center_matrix = matrix[center_pos].astype(np.float32)
                bucket = np.argmax((matrix[[positions[a] for a in indexed]].astype(np.float32) @ center_matrix.T).astype(np.float16), axis=1)
            else:
                torch = self._cuda_torch
                q = torch.as_tensor([positions[a] for a in indexed], device=matrix.device)
                c = torch.as_tensor(center_pos, device=matrix.device)
                with torch.inference_mode(): bucket = torch.argmax(matrix[q] @ matrix[c].T, dim=1).cpu().tolist()
            clusters = [[aid for aid, group in zip(indexed, bucket) if group == i] for i in range(len(centers))]
            clusters = [cluster for cluster in clusters if cluster]
        def chain(values: list[int]) -> list[int]:
            if len(values) < 3: return values[:]
            if isinstance(matrix, np.ndarray):
                data = matrix[[positions[a] for a in values]].astype(np.float32)
                similarities = (data @ data.T).astype(np.float16)
            else:
                torch = self._cuda_torch
                positions_tensor = torch.as_tensor([positions[a] for a in values], device=matrix.device)
                with torch.inference_mode(): similarities = (matrix[positions_tensor] @ matrix[positions_tensor].T).float().cpu().numpy()
            remaining = set(range(1, len(values))); result = [0]
            while remaining:
                current = result[-1]
                candidates = sorted(remaining)
                nxt = candidates[int(np.argmax(similarities[current, candidates]))]
                result.append(nxt); remaining.remove(nxt)
            return [values[i] for i in result]
        ordered_clusters = [chain(cluster) for cluster in clusters]
        if len(ordered_clusters) > 1:
            endpoints = [group[0] for group in ordered_clusters]
            if isinstance(matrix, np.ndarray):
                data = matrix[[positions[a] for a in endpoints]].astype(np.float32)
                cluster_scores = (data @ data.T).astype(np.float16)
            else:
                torch = self._cuda_torch
                endpoint_pos = torch.as_tensor([positions[a] for a in endpoints], device=matrix.device)
                with torch.inference_mode(): cluster_scores = (matrix[endpoint_pos] @ matrix[endpoint_pos].T).float().cpu().numpy()
            remaining_clusters = list(range(1, len(ordered_clusters)))
            cluster_order = [0]
            while remaining_clusters:
                current = cluster_order[-1]
                chosen = int(np.argmax(cluster_scores[current, remaining_clusters]))
                cluster_order.append(remaining_clusters.pop(chosen))
            ordered_clusters = [ordered_clusters[i] for i in cluster_order]
        return {"asset_ids": [aid for group in ordered_clusters for aid in group] + unindexed}

    def _collect_batch_groups(self, targets: list[int], candidates: list[int], scores: np.ndarray,
                              labels: dict[int, set[int]], parents: dict[int, int | None], minimum: float,
                              groups: dict[int, list[tuple[int, float]]]) -> None:
        candidate_positions = {candidate: position for position, candidate in enumerate(candidates)}
        for row, aid in enumerate(targets):
            similarities = scores[row].copy()
            if aid in candidate_positions:
                similarities[candidate_positions[aid]] = -np.inf
            take = min(SUGGEST_K, len(candidates))
            chosen = np.argpartition(-similarities, take - 1)[:take] if take else np.empty((0,), dtype=np.int64)
            chosen = chosen[np.argsort(-similarities[chosen], kind="stable")]
            neighbors = [(candidates[int(i)], float(similarities[i])) for i in chosen if np.isfinite(similarities[i])]
            ranked = self._rank_neighbors(aid, neighbors, 1, labels, parents, minimum)
            if ranked:
                item = ranked[0]
                groups.setdefault(item["label_id"], []).append((aid, item["score"]))

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


def _main(argv: list[str] | None = None) -> int:
    import argparse

    parser = argparse.ArgumentParser(description="DDreams local SigLIP2 JSON-lines server")
    parser.add_argument("--model", required=True, help="Lokálny priečinok modelu")
    parser.add_argument("--db", required=True, help="Cesta k databáze ai.sqlite")
    parser.add_argument("--lazy-model", action="store_true")
    args = parser.parse_args(argv)
    os.environ["HF_HUB_OFFLINE"] = "1"
    try:
        sys.stdin.reconfigure(encoding="utf-8", errors="replace")
        sys.stdout.reconfigure(encoding="utf-8", errors="replace", newline="\n")
        sys.stderr.reconfigure(encoding="utf-8", errors="backslashreplace")
    except (AttributeError, OSError):
        pass
    try:
        AIService(args.db, args.model, lazy_model=args.lazy_model).serve()
        return 0
    except Exception as exc:
        _stderr(f"Fatálna chyba servera: {exc}")
        return 1


if __name__ == "__main__":
    raise SystemExit(_main())


