"""CPU-only leave-one-out evaluation for DDreams label suggestions."""

from __future__ import annotations

import argparse
import json
import sqlite3
from pathlib import Path

import numpy as np


def readonly(path: str) -> sqlite3.Connection:
    return sqlite3.connect(Path(path).resolve().as_uri() + "?mode=ro", uri=True)


def ancestors(label: int, parents: dict[int, int | None]) -> set[int]:
    result = set()
    while label in parents and parents[label] is not None:
        label = int(parents[label])
        if label in result:
            break
        result.add(label)
    return result


def root_of(label: int, categories: dict[int, tuple[int | None, int | None]]) -> int:
    root_id = categories.get(label, (None, None))[1]
    if root_id:
        return int(root_id)
    seen = set()
    while label in categories and categories[label][0] is not None and label not in seen:
        seen.add(label)
        label = int(categories[label][0])
    return label


def evaluate(matrix: np.ndarray, ids: list[int], labels: dict[int, set[int]], parents: dict[int, int | None],
             ks: tuple[int, ...], roots: dict[int, int] | None = None) -> dict:
    asset_pos = {aid: i for i, aid in enumerate(ids)}
    query_ids = [aid for aid in ids if labels.get(aid)]
    candidate_ids = [aid for aid in ids if labels.get(aid)]
    candidate_pos = np.asarray([asset_pos[aid] for aid in candidate_ids], dtype=np.int64)
    candidate_col = {aid: i for i, aid in enumerate(candidate_ids)}
    candidate_matrix = matrix[candidate_pos]
    max_k = min(max(ks), max(0, len(candidate_ids) - 1))
    predictions = {}
    for start in range(0, len(query_ids), 128):
        batch_ids = query_ids[start:start + 128]
        query_pos = np.asarray([asset_pos[aid] for aid in batch_ids], dtype=np.int64)
        similarities = matrix[query_pos] @ candidate_matrix.T
        for row, aid in enumerate(batch_ids):
            if aid in asset_pos and aid in candidate_ids:
                similarities[row, candidate_col[aid]] = -np.inf
            take = min(max_k, len(candidate_ids))
            if take:
                idx = np.argpartition(-similarities[row], take - 1)[:take]
                idx = idx[np.argsort(-similarities[row, idx], kind="stable")]
                predictions[aid] = [(candidate_ids[int(j)], float(similarities[row, j])) for j in idx]
            else:
                predictions[aid] = []

    output = {}
    for k in ks:
        hit1 = hit3 = n = 0
        bucket = {str(t): {"covered": 0, "hits": 0} for t in (0.3, 0.5, 0.7, 0.9)}
        for aid in query_ids:
            truth = labels[aid]
            excluded = set()
            for label in tuple(truth):
                excluded.update(ancestors(label, parents))
            votes: dict[int, float] = {}
            denominator = 0.0
            for neighbor_id, similarity in predictions[aid][:k]:
                denominator += similarity
                for candidate in {roots[label] if roots else label for label in labels[neighbor_id]}:
                    votes[candidate] = votes.get(candidate, 0.0) + similarity
            actual = {roots[label] for label in truth} if roots else truth
            if roots:
                excluded = set()
            ranked = sorted(((label, score / denominator) for label, score in votes.items()
                             if denominator > 0 and label not in excluded), key=lambda item: (-item[1], item[0]))
            predicted = [label for label, _ in ranked]
            n += 1
            hit1 += bool(predicted[:1] and predicted[0] in actual)
            hit3 += bool(actual.intersection(predicted[:3]))
            if ranked:
                top, score = ranked[0]
                for threshold, values in ((float(t), v) for t, v in bucket.items()):
                    if score >= threshold:
                        values["covered"] += 1
                        values["hits"] += top in actual
        output[str(k)] = {
            "assets": n,
            "top1_hit_rate": hit1 / n if n else 0.0,
            "top3_hit_rate": hit3 / n if n else 0.0,
            "precision_buckets": {t: {"precision": row["hits"] / row["covered"] if row["covered"] else None,
                                       "coverage": row["covered"] / n if n else 0.0, "covered": row["covered"]}
                                  for t, row in bucket.items()},
        }
    return output


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ai-db", required=True)
    parser.add_argument("--catalog-db", required=True)
    parser.add_argument("--out", help="Optional JSON report path")
    args = parser.parse_args()
    with readonly(args.ai_db) as ai, readonly(args.catalog_db) as catalog:
        rows = ai.execute("SELECT asset_id,vector FROM embeddings ORDER BY asset_id").fetchall()
        ids = [int(aid) for aid, _ in rows]
        matrix = np.empty((len(rows), 1024), dtype=np.float32)
        for i, (_, blob) in enumerate(rows):
            vector = np.frombuffer(blob, dtype=np.float16).astype(np.float32)
            norm = np.linalg.norm(vector)
            matrix[i] = vector / max(float(norm), 1e-12)
        id_set = set(ids)
        categories = {int(i): (int(p) if p is not None else None, int(r) if r is not None else None)
                      for i, p, r in catalog.execute("SELECT id,parent_id,root_id FROM categories")}
        category_parents = {i: row[0] for i, row in categories.items()}
        root_map = {i: root_of(i, categories) for i in categories}
        keywords = {int(i): int(p) if p is not None else None for i, p in catalog.execute("SELECT id,parent_id FROM keywords")}
        result = {"snapshot": {"embeddings": len(ids), "note": "7,997 samples are far fewer than the full catalogue; snapshot indexing is partial."}}
        for kind, table, column, parents in (("category", "asset_categories", "category_id", category_parents),
                                            ("keyword", "asset_keywords", "keyword_id", keywords)):
            labels: dict[int, set[int]] = {}
            for aid, label in catalog.execute(f"SELECT ac.asset_id,ac.{column} FROM {table} ac JOIN assets a ON a.id=ac.asset_id WHERE a.is_listed=1"):
                aid, label = int(aid), int(label)
                if aid in id_set:
                    labels.setdefault(aid, set()).add(label)
            result[kind] = evaluate(matrix, ids, labels, parents, (5, 10, 20))
            result[kind]["labeled_indexed_assets"] = len(labels)
            if kind == "category":
                result["root_category"] = evaluate(matrix, ids, labels, parents, (5, 10, 20), root_map)
                result["root_category"]["labeled_indexed_assets"] = len(labels)
    rendered = json.dumps(result, ensure_ascii=False, indent=2)
    if args.out:
        Path(args.out).write_text(rendered + "\n", encoding="utf-8")
    print(rendered)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
