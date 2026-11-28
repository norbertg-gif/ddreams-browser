from __future__ import annotations

import io
from pathlib import Path
from typing import Any

from .constants import RAW_EXTENSIONS, RAW_MIN_EMBEDDED_PX
from .quality import compute_quality


def _decode_raw(path: str, decode_side: int):
    from PIL import Image, ImageOps
    import rawpy

    # Files with a RAW suffix can contain ordinary images. Preserve errors such as
    # LibRawTooBigError so the indexing worker can record them.
    try:
        raw_file = rawpy.imread(path)
    except rawpy.LibRawFileUnsupportedError:
        return None

    min_preview = min(decode_side, RAW_MIN_EMBEDDED_PX)
    with raw_file as raw:
        try:
            thumb = raw.extract_thumb()
        except (rawpy.LibRawNoThumbnailError, rawpy.LibRawUnsupportedThumbnailError):
            thumb = None

        image = None
        if thumb is not None and thumb.format == rawpy.ThumbFormat.JPEG:
            try:
                with Image.open(io.BytesIO(thumb.data)) as preview:
                    if max(preview.size) >= min_preview:
                        preview.draft("RGB", (decode_side, decode_side))
                        image = ImageOps.exif_transpose(preview).convert("RGB")
            except (OSError, ValueError, SyntaxError, Image.DecompressionBombError):
                pass
        elif thumb is not None and thumb.format == rawpy.ThumbFormat.BITMAP:
            pixels = thumb.data
            if pixels.ndim == 3 and pixels.shape[2] == 3 and max(pixels.shape[:2]) >= min_preview:
                image = Image.fromarray(pixels, "RGB")
            del pixels
        # Release the preview data before allocating the developed image.
        del thumb
        if image is None:
            image = Image.fromarray(raw.postprocess(half_size=True, use_camera_wb=True, output_bps=8))
    image.thumbnail((decode_side, decode_side), Image.Resampling.BICUBIC)
    return image


def decode_photo(item: dict[str, Any], max_side: int = 512):
    """Decode a photo to an oriented RGB PIL image suitable for model input."""
    from PIL import Image, ImageOps

    path = item["path"]
    ext = Path(path).suffix.lower().lstrip(".")
    decode_side = max(1024, max_side)
    if ext in RAW_EXTENSIONS:
        image = _decode_raw(path, decode_side)
        if image is not None:
            return _finish_decoded(image, max_side)

    with Image.open(path) as source:
        if ext in {"jpg", "jpeg", "png"} or (ext in RAW_EXTENSIONS and source.format in {"JPEG", "PNG"}):
            source.draft("RGB", (decode_side, decode_side))
        if ext in {"tif", "tiff"} or (ext in RAW_EXTENSIONS and source.format == "TIFF"):
            source.thumbnail((decode_side, decode_side), Image.Resampling.BICUBIC)
        image = ImageOps.exif_transpose(source).convert("RGB")
    return _finish_decoded(image, max_side)


def _finish_decoded(image: Any, max_side: int = 512):
    """Measure quality while the image is still up to 1024 px, then reduce it to the model input size."""
    from PIL import Image

    quality = compute_quality(image)
    image.thumbnail((max_side, max_side), Image.Resampling.BICUBIC)
    image.info["ddreams_quality"] = quality
    return image
