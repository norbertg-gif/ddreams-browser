from __future__ import annotations

import numpy as np


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
