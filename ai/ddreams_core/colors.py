from __future__ import annotations

from typing import Any
import numpy as np

from .constants import COLOR_PALETTE, COLOR_DEFAULTS


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
