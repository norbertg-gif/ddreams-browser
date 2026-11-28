from __future__ import annotations

import time
from pathlib import Path
from typing import Any
import numpy as np

from ..constants import (
    SUGGEST_K,
    CLUSTER_MIN_SIZE,
    CLUSTER_MIN_SIM,
    CLUSTER_IGNORE_SIM,
    CLUSTER_ITERATIONS,
    CLUSTER_VOCABULARY,
)
from ..util import RequestError


class ClustersMixin:
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
