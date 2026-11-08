"""Offline SigLIP2 phase-0 benchmark for the DDreams development catalog."""

from __future__ import annotations

import argparse
import concurrent.futures
import gc
import io
import json
import math
import os
import random
import sqlite3
import statistics
import subprocess
import sys
import time
import traceback
from pathlib import Path

# Set offline mode before importing transformers, huggingface_hub, or open_clip.
os.environ["HF_HUB_OFFLINE"] = "1"
os.environ.setdefault("HF_HOME", r"D:\voicestudio\models")
if hasattr(sys.stdout, "reconfigure"):
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")

import numpy as np
import open_clip
import rawpy
import torch
from PIL import Image, ImageOps
from open_clip.tokenizer import HFTokenizer
from safetensors.torch import load_file


ROOT = Path(__file__).resolve().parents[1]
CATALOG = ROOT / ".local" / "catalog.sqlite"
MODEL_DIR = Path(r"G:\DDreams\modules\ai\model")
FP16_CHECKPOINT = MODEL_DIR / "open_clip_model.fp16.safetensors"
FP32_CHECKPOINT = Path(r"D:\voicestudio\models\hub\models--timm--ViT-L-16-SigLIP2-256\snapshots\da9426945c7c5acbd3afd0c158c8bb9cfd4d8cc0\open_clip_model.safetensors")
EXIFTOOL = Path(r"G:\DDreams\modules\exiftool\exiftool.exe")
MODEL_NAME = "ViT-L-16-SigLIP2-256"
SEED = 20261108
LIMIT = 1000
TARGETS = {"jpeg_png": 600, "raw": 300, "tiff": 100}
EXT_GROUP = {
    **{ext: "jpeg_png" for ext in ("jpg", "jpeg", "png")},
    **{ext: "raw" for ext in ("arw", "dng", "cr2", "nef")},
    **{ext: "tiff" for ext in ("tif", "tiff")},
}
QUERIES = ["red poppy flower", "červený mak", "dog", "pes", "sunset over the sea", "západ slnka nad morom", "architecture", "portrait of a woman"]


def percentile95(values: list[float]) -> float | None:
    if not values:
        return None
    return sorted(values)[max(0, math.ceil(0.95 * len(values)) - 1)]


def category_data(conn: sqlite3.Connection):
    rows = conn.execute("SELECT id,parent_id,root_id,name FROM categories").fetchall()
    cats = {int(cid): {"parent": parent, "root": int(root) if root is not None else int(cid), "name": name} for cid, parent, root, name in rows}
    paths = {}
    for cid in cats:
        parts, cur, seen = [], cid, set()
        while cur in cats and cur not in seen:
            seen.add(cur)
            parts.append(cats[cur]["name"])
            cur = cats[cur]["parent"]
            if cur is None:
                break
        paths[cid] = "/".join(reversed(parts))
    return cats, paths


def sample_assets(conn: sqlite3.Connection):
    catalog_total = int(conn.execute("SELECT count(*) FROM assets WHERE is_listed=1").fetchone()[0])
    cats, cat_paths = category_data(conn)
    assignments: dict[int, list[int]] = {}
    for aid, cid in conn.execute("SELECT ac.asset_id,ac.category_id FROM asset_categories ac JOIN assets a ON a.id=ac.asset_id WHERE a.is_listed=1"):
        assignments.setdefault(int(aid), []).append(int(cid))
    candidates = {name: [] for name in TARGETS}
    rows = conn.execute("SELECT id,name,ext_lower,path,size FROM assets WHERE is_listed=1 AND path IS NOT NULL AND ext_lower IN (" + ",".join("?" for _ in EXT_GROUP) + ")", tuple(EXT_GROUP)).fetchall()
    for aid, name, ext, path, size in rows:
        if path and os.path.isfile(path):
            group = EXT_GROUP[ext.lower()]
            candidates[group].append({"id": int(aid), "name": name, "ext": ext.lower(), "path": path, "size": size, "categories": assignments.get(int(aid), [])})
    rng = random.Random(SEED)
    selected, remaining = [], {}
    for group in TARGETS:
        pool = candidates[group][:]
        rng.shuffle(pool)
        labeled = [item for item in pool if item["categories"]]
        unlabeled = [item for item in pool if not item["categories"]]
        quota = min(TARGETS[group], len(pool))
        take = labeled[:quota]
        if len(take) < quota:
            take.extend(unlabeled[:quota - len(take)])
        selected.extend(take)
        used = {item["id"] for item in take}
        remaining[group] = [item for item in pool if item["id"] not in used]
    if len(selected) < LIMIT:
        extra = [item for group in remaining.values() for item in group]
        rng.shuffle(extra)
        selected.extend(extra[: LIMIT - len(selected)])
    rng.shuffle(selected)
    return selected[:LIMIT], candidates, cats, cat_paths, catalog_total


def exiftool_preview(path: str) -> Image.Image:
    for tag in ("-JpgFromRaw", "-PreviewImage"):
        result = subprocess.run([str(EXIFTOOL), "-b", tag, path], stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, check=False)
        if result.returncode == 0 and result.stdout:
            try:
                image = Image.open(io.BytesIO(result.stdout))
                image.load()
                return image.convert("RGB")
            except Exception:
                continue
    raise ValueError("no decodable embedded preview (rawpy and ExifTool)")


def rawpy_preview(path: str) -> Image.Image:
    with rawpy.imread(path) as raw:
        thumb = raw.extract_thumb()
    if thumb.format == rawpy.ThumbFormat.JPEG:
        image = Image.open(io.BytesIO(thumb.data))
        image.load()
        return image.convert("RGB")
    if thumb.format == rawpy.ThumbFormat.BITMAP:
        return Image.fromarray(np.asarray(thumb.data).reshape(thumb.height, thumb.width, 3), "RGB")
    raise ValueError(f"unsupported rawpy thumbnail format: {thumb.format}")


def decode_asset(item: dict, raw_method: str = "rawpy"):
    started = time.perf_counter()
    try:
        if item["ext"] in {"arw", "dng", "cr2", "nef"}:
            try:
                image = rawpy_preview(item["path"]) if raw_method == "rawpy" else exiftool_preview(item["path"])
            except Exception:
                image = exiftool_preview(item["path"]) if raw_method == "rawpy" else rawpy_preview(item["path"])
            image.thumbnail((512, 512), Image.Resampling.BICUBIC)
        else:
            image = Image.open(item["path"])
            if item["ext"] in {"jpg", "jpeg"}:
                image.draft("RGB", (512, 512))
            if item["ext"] in {"tif", "tiff"}:
                image.thumbnail((512, 512), Image.Resampling.BICUBIC)
            image = ImageOps.exif_transpose(image)
            image.load()
            image = image.convert("RGB")
            image.thumbnail((512, 512), Image.Resampling.BICUBIC)
        image = ImageOps.exif_transpose(image).convert("RGB")
        return image, (time.perf_counter() - started) * 1000.0, None
    except Exception as exc:
        return None, (time.perf_counter() - started) * 1000.0, f"{type(exc).__name__}: {exc}"


def probe_raw_methods(paths: list[str]):
    paths = paths[:8]
    if not paths:
        return {"selected": "rawpy", "samples": 0, "rawpy_ms_per_file": None, "exiftool_ms_per_file": None}
    results = {}
    for method in ("rawpy", "exiftool"):
        times, failures = [], 0
        method_started = time.perf_counter()
        for path in paths:
            item = {"path": path, "ext": Path(path).suffix.lower().lstrip(".")}
            started = time.perf_counter()
            try:
                image = rawpy_preview(path) if method == "rawpy" else exiftool_preview(path)
                image.close()
                times.append(time.perf_counter() - started)
            except Exception:
                failures += 1
        total_seconds = time.perf_counter() - method_started
        results[method] = {"mean_ms_per_file": total_seconds * 1000 / len(paths) if paths else None, "successes": len(times), "failures": failures, "total_probe_seconds": total_seconds}
    winner = min((m for m in results if results[m]["mean_ms_per_file"] is not None), key=lambda m: results[m]["mean_ms_per_file"], default="rawpy")
    return {"selected": winner, "samples": len(paths), "rawpy": results["rawpy"], "exiftool": results["exiftool"]}


def make_tokenizer():
    config = open_clip.get_model_config(MODEL_NAME)
    text_cfg = config["text_cfg"]
    local = HFTokenizer(str(MODEL_DIR), context_length=text_cfg["context_length"], **text_cfg.get("tokenizer_kwargs", {}))
    reference = open_clip.get_tokenizer(MODEL_NAME)
    equal = torch.equal(local(QUERIES), reference(QUERIES))
    if not equal:
        raise RuntimeError("local tokenizer token IDs differ from open_clip.get_tokenizer")
    return local


def load_model(checkpoint: Path, use_fp16: bool):
    started = time.perf_counter()
    model, _, preprocess = open_clip.create_model_and_transforms(
        MODEL_NAME, device="cuda", image_mean=(0.5, 0.5, 0.5), image_std=(0.5, 0.5, 0.5),
        image_interpolation="bicubic", image_resize_mode="squash")
    model.eval()
    if use_fp16:
        model.half()
        for name, param in model.named_parameters():
            if name.endswith("logit_scale") or name.endswith("logit_bias"):
                param.data = param.data.float()
    state = load_file(str(checkpoint), device="cuda")
    incompat = model.load_state_dict(state, strict=False)
    del state
    torch.cuda.synchronize()
    return model, preprocess, time.perf_counter() - started, {"missing_keys": incompat.missing_keys, "unexpected_keys": incompat.unexpected_keys}


def encode_batch(model, batch_tensor):
    with torch.inference_mode(), torch.autocast("cuda"):
        features = model.encode_image(batch_tensor)
        features = features.float()
        return features / features.norm(dim=-1, keepdim=True).clamp_min(1e-12)


def gpu_throughput(model, preprocess, images):
    tensors = torch.stack([preprocess(im) for im in images]).to("cuda", non_blocking=False)
    results = {}
    for batch_size in (16, 32, 64):
        with torch.inference_mode(), torch.autocast("cuda"):
            _ = model.encode_image(tensors[:min(batch_size, len(images))])
        torch.cuda.synchronize()
        started = time.perf_counter()
        for offset in range(0, len(images), batch_size):
            _ = encode_batch(model, tensors[offset:offset + batch_size])
        torch.cuda.synchronize()
        elapsed = time.perf_counter() - started
        results[str(batch_size)] = {"images": len(images), "seconds": elapsed, "images_per_second": len(images) / elapsed}
    del tensors
    return results


def save_embeddings(path: Path, rows: list[tuple[int, str, int, int, int, bytes]]):
    conn = sqlite3.connect(path)
    conn.execute("CREATE TABLE embeddings(asset_id INTEGER PRIMARY KEY, model TEXT, size INTEGER, mtime_ns INTEGER, dim INTEGER, vector BLOB)")
    conn.executemany("INSERT INTO embeddings VALUES(?,?,?,?,?,?)", rows)
    conn.commit()
    conn.close()


def category_labels(item, cats, paths):
    return [paths.get(cid, str(cid)) for cid in item["categories"] if cid in cats]


def text_search(model, tokenizer, embeddings: np.ndarray, items, cats, paths):
    with torch.inference_mode(), torch.autocast("cuda"):
        text_tokens = tokenizer(QUERIES).to("cuda")
        features = model.encode_text(text_tokens).float()
        features = features / features.norm(dim=-1, keepdim=True).clamp_min(1e-12)
    scores = features.cpu().numpy() @ embeddings.T
    output = {}
    for qi, query in enumerate(QUERIES):
        top = np.argsort(-scores[qi])[:5]
        output[query] = [{"asset_id": items[i]["id"], "name": items[i]["name"], "similarity": float(scores[qi, i]), "categories": category_labels(items[i], cats, paths)} for i in top]
    return output


def knn_category_metrics(embeddings: np.ndarray, items, cats):
    indices = [i for i, item in enumerate(items) if item["categories"]]
    if len(indices) < 2:
        return {"n": len(indices), "category_top1": None, "category_top3": None, "root_top1": None, "root_top3": None}
    matrix = embeddings[indices].astype(np.float32)
    sims = matrix @ matrix.T
    np.fill_diagonal(sims, -np.inf)
    hit1 = hit3 = root1 = root3 = 0
    for row, global_i in enumerate(indices):
        nn = np.argpartition(-sims[row], min(7, len(indices) - 1) - 1)[:min(7, len(indices) - 1)]
        nn = nn[np.argsort(-sims[row, nn])]
        votes, root_votes = {}, {}
        true = set(items[global_i]["categories"])
        true_roots = {cats[cid]["root"] for cid in true if cid in cats}
        for ni in nn:
            weight = max(float(sims[row, ni]), 0.0)
            if weight == 0:
                weight = 1e-6
            neighbor = items[indices[int(ni)]]
            for cid in set(neighbor["categories"]):
                votes[cid] = votes.get(cid, 0.0) + weight
                if cid in cats:
                    rid = cats[cid]["root"]
                    root_votes[rid] = root_votes.get(rid, 0.0) + weight
        predicted = [cid for cid, _ in sorted(votes.items(), key=lambda pair: (-pair[1], pair[0]))]
        predicted_roots = [cid for cid, _ in sorted(root_votes.items(), key=lambda pair: (-pair[1], pair[0]))]
        hit1 += bool(predicted and predicted[0] in true)
        hit3 += bool(set(predicted[:3]) & true)
        root1 += bool(predicted_roots and predicted_roots[0] in true_roots)
        root3 += bool(set(predicted_roots[:3]) & true_roots)
    n = len(indices)
    return {"n": n, "category_top1": hit1 / n, "category_top3": hit3 / n, "root_top1": root1 / n, "root_top3": root3 / n, "k": 7, "vote": "sum(max(cosine, 0); zero scores use 1e-6)", "note": "1000 sampled images are far fewer than the full catalog."}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", required=True, type=Path)
    args = parser.parse_args()
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=True)
    report_path, embeddings_path = out / "report.json", out / "embeddings.sqlite"
    for path in (report_path, embeddings_path):
        if path.exists():
            raise FileExistsError(f"refusing to overwrite existing output: {path}")
    if not torch.cuda.is_available():
        raise RuntimeError("CUDA is required for this benchmark")
    for path in (CATALOG, FP16_CHECKPOINT, FP32_CHECKPOINT, MODEL_DIR):
        if not path.exists():
            raise FileNotFoundError(path)

    conn = sqlite3.connect(f"file:{CATALOG.as_posix()}?mode=ro", uri=True)
    conn.execute("PRAGMA query_only=ON")
    sample, candidates, cats, paths, catalog_total = sample_assets(conn)
    conn.close()
    if not sample:
        raise RuntimeError("no eligible listed assets with existing files were found")
    raw_probe = probe_raw_methods([item["path"] for item in sample if item["ext"] in {"arw", "dng", "cr2", "nef"}])
    raw_method = raw_probe["selected"]
    print(f"Eligible sample: {len(sample)} assets; RAW preview probe selected {raw_method}.", flush=True)

    decoded_cache = []
    first_started = time.perf_counter()
    with concurrent.futures.ThreadPoolExecutor(max_workers=6, thread_name_prefix="phase0-decode") as pool:
        futures = [pool.submit(decode_asset, item, raw_method) for item in sample[:256]]
        for item, future in zip(sample[:256], futures):
            image, ms, error = future.result()
            if not error:
                decoded_cache.append((item, image))
    del futures
    decode_first_256_seconds = time.perf_counter() - first_started
    if len(decoded_cache) < 64:
        raise RuntimeError(f"only {len(decoded_cache)} of first 256 assets decoded; at least 64 are needed")

    tokenizer = make_tokenizer()
    model, preprocess, model_load_seconds, state_info = load_model(FP16_CHECKPOINT, True)
    torch.cuda.reset_peak_memory_stats()
    throughput_images = [image for _, image in decoded_cache[:256]]
    throughput = gpu_throughput(model, preprocess, throughput_images)
    fidelity_images = [image for _, image in decoded_cache[:64]]
    del decoded_cache, throughput_images
    gc.collect()

    # Decode afresh while batches are embedded so this wall time includes I/O, decode, preprocessing and inference.
    main_started = time.perf_counter()
    all_embeddings = []
    successful_items = []
    errors = {group: {} for group in TARGETS}
    group_times = {group: [] for group in TARGETS}
    with concurrent.futures.ThreadPoolExecutor(max_workers=6, thread_name_prefix="phase0-decode") as pool:
        for start in range(0, len(sample), 32):
            chunk = sample[start:start + 32]
            futures = [pool.submit(decode_asset, item, raw_method) for item in chunk]
            batch_items, batch_images = [], []
            for item, future in zip(chunk, futures):
                image, ms, error = future.result()
                group = EXT_GROUP[item["ext"]]
                group_times[group].append(ms)
                if error:
                    errors[group][error] = errors[group].get(error, 0) + 1
                else:
                    batch_items.append(item)
                    batch_images.append(image)
            for offset in range(0, len(batch_images), 32):
                chunk_items = batch_items[offset:offset + 32]
                tensors = torch.stack([preprocess(im) for im in batch_images[offset:offset + 32]]).to("cuda")
                features = encode_batch(model, tensors).cpu().numpy().astype(np.float32)
                all_embeddings.extend(features)
                successful_items.extend(chunk_items)
                del tensors
    end_to_end_seconds = time.perf_counter() - main_started
    embedding_matrix = np.asarray(all_embeddings, dtype=np.float32)
    if not len(embedding_matrix):
        raise RuntimeError("no images could be embedded")
    torch.cuda.synchronize()
    peak_vram_gib = torch.cuda.max_memory_allocated() / (1024 ** 3)
    pure_gpu = throughput["32"]["images_per_second"]
    end_to_end_fps = len(successful_items) / end_to_end_seconds

    text_results = text_search(model, tokenizer, embedding_matrix, successful_items, cats, paths)
    knn_results = knn_category_metrics(embedding_matrix, successful_items, cats)

    db_rows = []
    vector_nbytes = None
    for item, vector in zip(successful_items, embedding_matrix):
        normed = vector / max(float(np.linalg.norm(vector)), 1e-12)
        payload = normed.astype(np.float16).tobytes()
        vector_nbytes = len(payload)
        stat = os.stat(item["path"])
        db_rows.append((item["id"], MODEL_NAME, 256, stat.st_mtime_ns, len(normed), payload))
    save_embeddings(embeddings_path, db_rows)

    fp16_fidelity_embeddings = []
    # Recompute with same model pipeline for a paired fp16-vs-fp32 comparison.
    for start in range(0, len(fidelity_images), 32):
        tensors = torch.stack([preprocess(im) for im in fidelity_images[start:start + 32]]).to("cuda")
        fp16_fidelity_embeddings.extend(encode_batch(model, tensors).cpu().numpy())
        del tensors
    del model
    gc.collect()
    torch.cuda.empty_cache()
    fp32_model, fp32_preprocess, fp32_load_seconds, fp32_state_info = load_model(FP32_CHECKPOINT, False)
    fp32_fidelity_embeddings = []
    for start in range(0, len(fidelity_images), 32):
        tensors = torch.stack([fp32_preprocess(im) for im in fidelity_images[start:start + 32]]).to("cuda")
        fp32_fidelity_embeddings.extend(encode_batch(fp32_model, tensors).cpu().numpy())
        del tensors
    f16 = np.asarray(fp16_fidelity_embeddings, dtype=np.float32)
    f32 = np.asarray(fp32_fidelity_embeddings, dtype=np.float32)
    cosines = np.sum(f16 * f32, axis=1) / (np.linalg.norm(f16, axis=1) * np.linalg.norm(f32, axis=1)).clip(1e-12)
    del fp32_model
    gc.collect()
    torch.cuda.empty_cache()

    decode_summary = {}
    for group, values in group_times.items():
        decode_summary[group] = {"count": len(values), "mean_ms_per_image": statistics.mean(values) if values else None, "p95_ms_per_image": percentile95(values), "failures": sum(errors[group].values()), "failure_reasons": errors[group]}
    report = {
        "benchmark": {"model": MODEL_NAME, "image_size": 256, "seed": SEED, "requested_limit": LIMIT, "sampled_count": len(sample), "embedded_count": len(successful_items), "catalog_total_listed": catalog_total, "note": "The 1000-image sample is far fewer than the full catalog.", "sampled_with_categories": sum(bool(item["categories"]) for item in sample), "sampled_without_categories": sum(not item["categories"] for item in sample), "eligible_existing_files_by_group": {group: len(rows) for group, rows in candidates.items()}, "sampled_by_group": {group: sum(EXT_GROUP[item["ext"]] == group for item in sample) for group in TARGETS}, "target_by_group": TARGETS},
        "tokenizer": {"source": str(MODEL_DIR), "reference": "open_clip.get_tokenizer(ViT-L-16-SigLIP2-256)", "token_ids_identical_for_sanity_queries": True, "context_length": 64, "clean": "canonicalize"},
        "raw_preview_probe": raw_probe,
        "model": {"fp16_checkpoint": str(FP16_CHECKPOINT), "fp16_load_seconds": model_load_seconds, "fp16_state_missing_keys": state_info["missing_keys"], "fp16_state_unexpected_keys": state_info["unexpected_keys"], "fp32_checkpoint": str(FP32_CHECKPOINT), "fp32_load_seconds_for_fidelity": fp32_load_seconds, "fp32_state_missing_keys": fp32_state_info["missing_keys"], "fp32_state_unexpected_keys": fp32_state_info["unexpected_keys"]},
        "decode": {"first_256_decode_wall_seconds": decode_first_256_seconds, "groups": decode_summary},
        "throughput": {"gpu_only_batch_size_trials": throughput, "pure_gpu_images_per_second_batch32": pure_gpu, "end_to_end_images_per_second": end_to_end_fps, "end_to_end_seconds": end_to_end_seconds, "peak_vram_gib": peak_vram_gib},
        "fp16_fidelity": {"paired_images": int(len(cosines)), "cosine_similarity_min": float(np.min(cosines)), "cosine_similarity_mean": float(np.mean(cosines)), "comparison": "fp16 checkpoint vs original fp32 checkpoint; same 64 decoded images and CUDA autocast inference"},
        "embeddings": {"database": str(embeddings_path), "table": "embeddings", "rows": len(db_rows), "dimension": int(embedding_matrix.shape[1]), "dtype": "float16", "l2_normalized_before_cast": True, "bytes_per_embedding": vector_nbytes, "database_file_bytes": embeddings_path.stat().st_size},
        "text_search_sanity": text_results,
        "knn_category_sanity": knn_results,
    }
    report_path.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
    print("\nPhase-0 SigLIP2 benchmark")
    print(f"Embedded: {len(successful_items)}/{len(sample)} | GPU batch32: {pure_gpu:.1f} img/s | end-to-end: {end_to_end_fps:.1f} img/s")
    print(f"Peak allocated VRAM: {peak_vram_gib:.2f} GiB | fp16 cosine min/mean: {np.min(cosines):.6f}/{np.mean(cosines):.6f}")
    print(f"kNN category top-1/top-3: {knn_results['category_top1']:.3f}/{knn_results['category_top3']:.3f} | top-level: {knn_results['root_top1']:.3f}/{knn_results['root_top3']:.3f}")
    print("\nText-search top-5:")
    for query, results in text_results.items():
        print(f"  {query}:")
        for rank, result in enumerate(results, 1):
            labels = "; ".join(result["categories"]) if result["categories"] else "(uncategorized)"
            print(f"    {rank}. {result['name']} [{labels}] ({result['similarity']:.4f})")
    print(f"Wrote: {report_path} and {embeddings_path}")


if __name__ == "__main__":
    try:
        main()
    except Exception:
        traceback.print_exc()
        raise
