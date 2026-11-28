from __future__ import annotations

from typing import Any
import numpy as np


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
