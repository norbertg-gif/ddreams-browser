from __future__ import annotations

import gc
import os
import threading
from pathlib import Path
from typing import Any
import numpy as np

from .constants import MODEL_NAME, DIMENSION, SIMILARITY_COSINE_FLOOR
from .util import text_probability_cutoff


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
