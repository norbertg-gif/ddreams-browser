"""DDreams RAW preview plugin: renders previews of RAW files that have no usable embedded preview.

Protocol: one JSON object per line on stdin, one per line on stdout (UTF-8).
  request  {"id": 1, "path": "F:\\a.dng", "size": 480}
  response {"id": 1, "ok": true, "jpeg": "<base64>", "source": "embedded"|"develop", "width": w, "height": h}
           {"id": 1, "ok": false, "error": "..."}
Requests are served concurrently (LibRaw releases the GIL while decoding). Exits on stdin EOF.
Logs go to stderr only.
"""

from __future__ import annotations

import base64
import concurrent.futures
import io
import json
import sys
import threading

import rawpy
from PIL import Image, ImageOps

MIN_EMBEDDED_PX = 800  # an embedded JPEG at least this large is good enough for a preview
JPEG_QUALITY = 85
WORKERS = 3


def render(path: str, size: int) -> dict:
    target = max(48, min(int(size or 2048), 4096))
    with rawpy.imread(path) as raw:
        image = None
        source = "develop"
        try:
            thumb = raw.extract_thumb()
            if thumb.format == rawpy.ThumbFormat.JPEG:
                candidate = ImageOps.exif_transpose(Image.open(io.BytesIO(thumb.data)))
                if max(candidate.size) >= min(target, MIN_EMBEDDED_PX):
                    image, source = candidate.convert("RGB"), "embedded"
        except (rawpy.LibRawNoThumbnailError, rawpy.LibRawUnsupportedThumbnailError):
            pass
        if image is None:
            # half_size skips demosaicing interpolation: 4x faster and plenty for previews.
            image = Image.fromarray(raw.postprocess(half_size=True, use_camera_wb=True, output_bps=8))
    image.thumbnail((target, target), Image.Resampling.LANCZOS)
    out = io.BytesIO()
    image.save(out, "JPEG", quality=JPEG_QUALITY)
    return {"jpeg": base64.b64encode(out.getvalue()).decode("ascii"), "source": source, "width": image.width, "height": image.height}


def main() -> None:
    stdin = io.TextIOWrapper(sys.stdin.buffer, encoding="utf-8")
    stdout = io.TextIOWrapper(sys.stdout.buffer, encoding="utf-8", newline="\n")
    lock = threading.Lock()

    def reply(message: dict) -> None:
        with lock:
            stdout.write(json.dumps(message, ensure_ascii=False) + "\n")
            stdout.flush()

    def handle(request: dict) -> None:
        rid = request.get("id")
        try:
            reply({"id": rid, "ok": True, **render(str(request["path"]), int(request.get("size") or 2048))})
        except Exception as exc:  # one bad file must never kill the plugin
            reply({"id": rid, "ok": False, "error": f"{type(exc).__name__}: {exc}"})

    with concurrent.futures.ThreadPoolExecutor(max_workers=WORKERS) as pool:
        for line in stdin:
            line = line.strip()
            if not line:
                continue
            try:
                request = json.loads(line)
            except ValueError as exc:
                reply({"id": None, "ok": False, "error": f"Neplatný JSON: {exc}"})
                continue
            pool.submit(handle, request)


if __name__ == "__main__":
    main()
