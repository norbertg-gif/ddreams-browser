from __future__ import annotations

from typing import Any
import numpy as np

from .constants import FACE_TEMPLATE


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
