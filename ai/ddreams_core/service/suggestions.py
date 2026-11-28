from __future__ import annotations

from typing import Any
import numpy as np

from ..constants import DIMENSION, SUGGEST_K, MIN_SCORE, AUTO_GROUP_SCORE, SUGGEST_BATCH_CHUNK, KEYWORD_VOCABULARY
from ..util import RequestError


class SuggestionsMixin:
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
