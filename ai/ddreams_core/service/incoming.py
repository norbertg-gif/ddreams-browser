from __future__ import annotations

from datetime import date
import ntpath
import os
import sqlite3
import time
from pathlib import Path
from typing import Any
import numpy as np

from ..util import RequestError, _stderr


class IncomingMixin:
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
