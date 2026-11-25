import base64
import io
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from types import SimpleNamespace
from unittest.mock import MagicMock, patch

from PIL import Image

from ddreams_raw import render, render_full

try:
    import pillow_heif
except ImportError:
    pillow_heif = None


class ImagePreviewTests(unittest.TestCase):
    def test_pillow_formats_render_for_the_grid_and_viewer(self):
        with tempfile.TemporaryDirectory() as tmp:
            image = Image.new("RGB", (640, 480), (230, 20, 30))
            for ext, format_name in (
                ("heic", "HEIF"), ("heif", "HEIF"), ("avif", "AVIF"),
                ("jp2", "JPEG2000"), ("j2k", "JPEG2000"), ("pcx", "PCX"),
            ):
                with self.subTest(ext=ext):
                    path = Path(tmp) / f"náhľad.{ext}"
                    if format_name == "HEIF":
                        if pillow_heif is None:
                            self.skipTest("pillow_heif is not installed")
                        pillow_heif.from_pillow(image).save(path)
                    else:
                        options = {"no_jp2": True} if ext == "j2k" else {}
                        image.save(path, format=format_name, **options)
                    for target, expected in ((160, (160, 120)), (2048, (640, 480))):
                        result = render(str(path), target, "image")
                        self.assertEqual(result["source"], "pillow")
                        self.assertEqual((result["width"], result["height"]), expected)
                        with Image.open(io.BytesIO(base64.b64decode(result["jpeg"]))) as output:
                            self.assertEqual((output.format, output.size), ("JPEG", expected))
                            self.assertGreater(output.getpixel((10, 10))[0], 200)

    def test_protocol_uses_group_and_retries_jp2_without_resolution_levels(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "jedna-vrstva.jp2"
            Image.new("RGB", (512, 384), "green").save(path, num_resolutions=1)
            request = {"id": 3, "path": str(path), "size": 160, "group": "image"}
            plugin = Path(__file__).with_name("ddreams_raw.py")
            result = subprocess.run([sys.executable, "-I", "-B", str(plugin)],
                                    input=json.dumps(request) + "\n", encoding="utf-8",
                                    capture_output=True, check=True, timeout=30)
            reply = json.loads(result.stdout)
            self.assertTrue(reply["ok"], reply)
            self.assertEqual((reply["id"], reply["width"], reply["height"]), (3, 160, 120))


class FullImageTests(unittest.TestCase):
    def test_pillow_full_protocol_preserves_dimensions_and_exif_orientation(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "wide.png"
            exif = Image.Exif()
            exif[274] = 6
            Image.new("RGB", (4200, 12), "red").save(path, exif=exif)
            request = {"id": 7, "path": str(path), "mode": "image"}
            plugin = Path(__file__).with_name("ddreams_raw.py")
            result = subprocess.run([sys.executable, "-I", "-B", str(plugin)],
                                    input=json.dumps(request) + "\n", encoding="utf-8",
                                    capture_output=True, check=True, timeout=30)
            reply = json.loads(result.stdout)
            self.assertTrue(reply["ok"], reply)
            self.assertEqual((reply["id"], reply["source"]), (7, "pillow"))
            self.assert_frame(reply, (12, 4200), (12, 4200))

    def setUp(self):
        self.raw = MagicMock()
        self.raw.sizes = SimpleNamespace(width=6000, height=4000, flip=6)
        self.raw.__enter__.return_value = self.raw
        self.srgb = object()
        self.no_thumbnail = type("NoThumbnail", (Exception,), {})
        self.jpeg_format = object()
        module = SimpleNamespace(imread=MagicMock(return_value=self.raw),
                                 ThumbFormat=SimpleNamespace(JPEG=self.jpeg_format),
                                 ColorSpace=SimpleNamespace(sRGB=self.srgb),
                                 LibRawNoThumbnailError=self.no_thumbnail,
                                 LibRawUnsupportedThumbnailError=type("UnsupportedThumb", (Exception,), {}))
        rawpy_patch = patch("ddreams_raw.rawpy", module)
        rawpy_patch.start()
        self.addCleanup(rawpy_patch.stop)

    def assert_frame(self, result, shown, original):
        self.assertEqual((result["width"], result["height"]), shown)
        self.assertEqual((result["originalWidth"], result["originalHeight"]), original)
        with Image.open(io.BytesIO(base64.b64decode(result["jpeg"]))) as output:
            self.assertEqual((output.format, output.size), ("JPEG", shown))

    def test_raw_quality_develops_full_color_without_extracting_preview(self):
        with patch("ddreams_raw.Image.fromarray", return_value=Image.new("RGB", (12, 4200))):
            result = render_full("unused.dng", "rawFull")
        self.raw.extract_thumb.assert_not_called()
        self.raw.postprocess.assert_called_once_with(half_size=False, use_camera_wb=True,
                                                     output_bps=8, output_color=self.srgb)
        self.assertEqual(result["source"], "rawFull")
        self.assert_frame(result, (12, 4200), (12, 4200))

    def test_raw_embedded_preview_keeps_native_size_and_applies_orientation(self):
        jpeg = io.BytesIO()
        Image.new("RGB", (4200, 12)).save(jpeg, "JPEG")
        self.raw.extract_thumb.return_value = SimpleNamespace(format=self.jpeg_format, data=jpeg.getvalue())
        result = render_full("unused.dng", "embedded")
        self.raw.postprocess.assert_not_called()
        self.assertEqual(result["source"], "embedded")
        self.assert_frame(result, (12, 4200), (4000, 6000))

    def test_raw_without_embedded_jpeg_uses_half_size(self):
        self.raw.extract_thumb.side_effect = self.no_thumbnail
        with patch("ddreams_raw.Image.fromarray", return_value=Image.new("RGB", (12, 20))):
            result = render_full("unused.dng", "embedded")
        self.raw.postprocess.assert_called_once_with(half_size=True, use_camera_wb=True,
                                                     output_bps=8, output_color=self.srgb)
        self.assertEqual(result["source"], "rawReduced")
        self.assert_frame(result, (12, 20), (4000, 6000))


if __name__ == "__main__":
    unittest.main()
