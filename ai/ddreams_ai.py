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

from ddreams_core.constants import (
    MODEL_NAME,
    DIMENSION,
    SCHEMA_VERSION,
    COLOR_VERSION,
    QUALITY_VERSION,
    DECODER_VERSION,
    IMAGE_RETRY_EXTENSIONS,
    RAW_EXTENSIONS,
    RAW_RETRY_ERRORS,
    RAW_MIN_EMBEDDED_PX,
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
    FACE_TEMPLATE,
    TEXT_PROB_FLOOR,
    TEXT_PROB_RELATIVE,
    SIMILARITY_COSINE_FLOOR,
    SUGGEST_K,
    MIN_SCORE,
    AUTO_GROUP_SCORE,
    DUP_SAME,
    DUP_SIMILAR,
    SUGGEST_BATCH_CHUNK,
    CLUSTER_MIN_SIZE,
    CLUSTER_MIN_SIM,
    CLUSTER_IGNORE_SIM,
    CLUSTER_ITERATIONS,
    MAP_ITERATIONS,
    MAP_RADIUS_SCALE,
    MAP_PACK_DENSITY,
    MAP_PACK_GAP,
    MAP_PACK_ITERATIONS,
    MAP_TSNE_ITERATIONS,
    MAP_TSNE_EXAGGERATION,
    MAP_TSNE_SEED,
    MAP_GOLDEN_ANGLE,
    CLUSTER_VOCABULARY,
    KEYWORD_VOCABULARY,
    BATCH_SIZE,
    DECODE_THREADS,
    CHROMATIC_COLORS,
    NEUTRAL_COLORS,
    COLOR_PALETTE,
    COLOR_DEFAULTS,
)
from ddreams_core.util import (
    text_probability_cutoff,
    RequestError,
    _stderr,
    _set_background_priority,
    process_memory,
)
from ddreams_core.colors import _hsv, palette_counts, dominant_colors, learn_thresholds
from ddreams_core.decode import _decode_raw, decode_photo, _finish_decoded
from ddreams_core.quality import compute_quality
from ddreams_core.face_models import face_detect, face_align, _box_iou
from ddreams_core.embedder import SiglipEmbedder
from ddreams_core.vector_index import VectorIndex
from ddreams_core.service import AIService


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
