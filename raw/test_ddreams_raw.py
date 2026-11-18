import base64
import io
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

import pillow_heif
from PIL import Image

from ddreams_raw import render


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


if __name__ == "__main__":
    unittest.main()
