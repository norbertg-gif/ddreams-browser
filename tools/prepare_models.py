"""Downloads and prepares the AI models DDreams uses (they are not stored in git).

Run with the DDreams Python (created by tools/setup-python.ps1):

    C:\\DDreams\\modules\\python\\Scripts\\python.exe tools\\prepare_models.py --modules C:\\DDreams\\modules

* SigLIP2 ViT-L-16-256 (timm/ViT-L-16-SigLIP2-256, Apache-2.0) from Hugging Face at a pinned
  revision, converted to fp16 (logit_scale/logit_bias stay fp32) -> modules/ai/model
* InsightFace buffalo_l detector + recogniser (non-commercial research licence) -> modules/ai/faces

Existing files with the expected checksum are kept. Nothing outside --modules is written
except the Hugging Face download cache.
"""
from __future__ import annotations

import argparse
import hashlib
import io
import shutil
import sys
import urllib.request
import zipfile
from pathlib import Path

SIGLIP_REPO = "timm/ViT-L-16-SigLIP2-256"
SIGLIP_REVISION = "da9426945c7c5acbd3afd0c158c8bb9cfd4d8cc0"
SIGLIP_FP16 = "open_clip_model.fp16.safetensors"
KEEP_FP32 = {"logit_scale", "logit_bias"}
TOKENIZER_FILES = {
    "special_tokens_map.json": "d4671afd8f9b1ccd170443bd5478c0cc40fdbddc9343034caf6984ffd388b2c4",
    "tokenizer.json": "220c63d496e0c14e63eb656c91e0215e926202e4c74b1f089e09f1920d779b04",
    "tokenizer_config.json": "513e778583a9ac0b74ae47784216f9d654d5132eee62d777597f5e842701323c",
}
FACES_URL = "https://github.com/deepinsight/insightface/releases/download/v0.7/buffalo_l.zip"
FACE_FILES = {
    "det_10g.onnx": "5838f7fe053675b1c7a08b633df49e7af5495cee0493c7dcf6697200b85b5b91",
    "w600k_r50.onnx": "4c06341c33c2ca1f86781dab0e829f88ad5b64be9fba56e56bc9ebdefc619e43",
}


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def convert_fp16(source: Path, target: Path) -> None:
    """Writes an fp16 copy of an open_clip safetensors checkpoint (tensor by tensor)."""
    import numpy as np
    from safetensors import safe_open
    from safetensors.numpy import save_file

    tensors = {}
    with safe_open(str(source), framework="numpy") as weights:
        for key in weights.keys():
            value = weights.get_tensor(key)
            if key not in KEEP_FP32 and np.issubdtype(value.dtype, np.floating):
                value = value.astype(np.float16)
            tensors[key] = value
    temporary = target.with_suffix(".tmp")
    save_file(tensors, str(temporary), metadata={"source": f"{SIGLIP_REPO}@{SIGLIP_REVISION[:7]}", "dtype": "fp16"})
    temporary.replace(target)


def prepare_siglip(model_dir: Path) -> None:
    from huggingface_hub import hf_hub_download

    model_dir.mkdir(parents=True, exist_ok=True)
    for name, expected in TOKENIZER_FILES.items():
        target = model_dir / name
        if target.is_file() and sha256(target) == expected:
            continue
        shutil.copyfile(hf_hub_download(SIGLIP_REPO, name, revision=SIGLIP_REVISION), target)
        if sha256(target) != expected:
            raise SystemExit(f"Checksum mismatch: {target}")
    target = model_dir / SIGLIP_FP16
    if target.is_file():
        print(f"ok   {target}")
    else:
        print("downloading SigLIP2 (~3.5 GB fp32) ...")
        source = Path(hf_hub_download(SIGLIP_REPO, "open_clip_model.safetensors", revision=SIGLIP_REVISION))
        convert_fp16(source, target)
        print(f"done {target}")


def prepare_faces(face_dir: Path) -> None:
    face_dir.mkdir(parents=True, exist_ok=True)
    missing = [name for name, expected in FACE_FILES.items()
               if not (face_dir / name).is_file() or sha256(face_dir / name) != expected]
    if not missing:
        print(f"ok   {face_dir}")
        return
    print("downloading InsightFace buffalo_l (~290 MB) ...")
    with urllib.request.urlopen(FACES_URL) as response:
        archive = zipfile.ZipFile(io.BytesIO(response.read()))
    for name in missing:
        member = next(item for item in archive.namelist() if item.rsplit("/", 1)[-1] == name)
        (face_dir / name).write_bytes(archive.read(member))
        if sha256(face_dir / name) != FACE_FILES[name]:
            raise SystemExit(f"Checksum mismatch: {face_dir / name}")
    print(f"done {face_dir}")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--modules", type=Path, default=Path(r"C:\DDreams\modules"))
    parser.add_argument("--skip-faces", action="store_true")
    args = parser.parse_args(argv)
    prepare_siglip(args.modules / "ai" / "model")
    if not args.skip_faces:
        prepare_faces(args.modules / "ai" / "faces")
    return 0


if __name__ == "__main__":
    sys.exit(main())
