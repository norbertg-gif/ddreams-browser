from __future__ import annotations

import time
from typing import Any
import numpy as np

from ..constants import (
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
)


class MapViewMixin:
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
