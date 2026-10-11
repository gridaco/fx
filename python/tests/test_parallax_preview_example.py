"""Provider-free acceptance checks for the looping-parallax preview example.

Only the encoded smoke test requires locally installed ffmpeg and ffprobe.
Geometry, transparency, limits and cancellation checks require neither tool.
"""

from __future__ import annotations

import hashlib
import importlib.util
import json
import shutil
import sys
import tempfile
import unittest
from pathlib import Path

from PIL import Image

from grida.fx import Ctx, NodeFailure

REPO = Path(__file__).resolve().parents[2]
SOURCE = REPO / "examples/looping-parallax/nodes/render_preview.py"
SPEC = importlib.util.spec_from_file_location("parallax_preview_example_under_test", SOURCE)
assert SPEC and SPEC.loader
MODULE = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = MODULE
SPEC.loader.exec_module(MODULE)


class Channel:
    def __init__(self):
        self.notifications = []

    def notify(self, method, params):
        self.notifications.append((method, params))


def manifest():
    return {
        "canvas": {"width": 256, "height": 128},
        "layers": [{"id": "tile", "order": 0, "offset_y": 0, "parallax": 0.85}],
    }


def parameters():
    return {"width": 256, "fps": 12, "duration_seconds": 2, "layer_wraps": {"tile": 1}}


def file_ref(path, kind):
    data = path.read_bytes()
    return {
        "path": str(path),
        "kind": kind,
        "size": len(data),
        "digest": hashlib.sha256(data).hexdigest(),
    }


def context(root):
    tile = Image.new("RGBA", (32, 128))
    for x in range(32):
        for y in range(128):
            tile.putpixel((x, y), (x * 8, y * 2, 80, 255))
    tile_path = root / "tile.png"
    tile.save(tile_path)
    manifest_path = root / "manifest.json"
    manifest_path.write_text(json.dumps(manifest()))
    work = root / "work"
    work.mkdir()
    return Ctx(
        {
            "instance": {"id": "render#1", "path": "render", "step": "render", "take": [1]},
            "params": parameters(),
            "inputs": {
                "layers": {"collection": [["tile", file_ref(tile_path, "image/png")]]},
                "manifest": file_ref(manifest_path, "json"),
            },
            "work_dir": str(work),
            # Cancellation happens before spawning any tool. A harmless existing
            # executable lets that lifecycle check run without optional codecs.
            "tools": {name: shutil.which(name) or sys.executable for name in ("ffmpeg", "ffprobe")},
        },
        Channel(),
    )


class PreviewTests(unittest.TestCase):
    def test_preview_declares_required_local_tools_and_no_paid_calls(self):
        spec = MODULE.render_preview.fx_spec
        self.assertEqual(set(spec.tools), {"ffmpeg", "ffprobe"})
        self.assertEqual(spec.calls, {})
        self.assertEqual(spec.resources, ())

    def test_forward_phase_has_constant_increments_including_last_to_first(self):
        settings = MODULE.settings_for(manifest(), parameters())
        for period in (960, 1920):
            for wraps in (1, 3, 16):
                phases = [
                    MODULE.layer_phase(index, period, wraps, settings.frame_count)
                    for index in range(settings.frame_count + 1)
                ]
                expected_step = period * wraps / settings.frame_count
                self.assertEqual(phases[0], 0)
                self.assertEqual(phases[-1], 0)
                self.assertGreater(phases[-2], 0)
                for index in range(settings.frame_count):
                    actual_step = (phases[index + 1] - phases[index]) % period
                    self.assertAlmostEqual(actual_step, expected_step)
                    self.assertGreater(actual_step, 0)

    def test_one_one_three_wraps_respect_different_native_periods(self):
        scene = {
            "canvas": {"width": 1536, "height": 1024},
            "layers": [
                {"id": "far", "order": 0, "offset_y": 0, "parallax": 0.12},
                {"id": "middle", "order": 1, "offset_y": 0, "parallax": 0.4},
                {"id": "foreground", "order": 2, "offset_y": 0, "parallax": 0.85},
            ],
        }
        settings = MODULE.settings_for(
            scene,
            {
                "width": 960,
                "fps": 24,
                "duration_seconds": 16,
                "layer_wraps": {"far": 1, "middle": 1, "foreground": 3},
            },
        )
        self.assertEqual(settings.width * settings.height * settings.frame_count, 235929600)
        pictures = {
            "far": Image.new("RGBA", (1536, 1024)),
            "middle": Image.new("RGBA", (3072, 1024)),
            "foreground": Image.new("RGBA", (1536, 1024)),
        }
        layers = MODULE.prepare_layers(scene, pictures, settings)
        self.assertEqual([layer.period for layer in layers], [960, 1920, 960])
        self.assertEqual([layer.wraps for layer in layers], [1, 1, 3])
        self.assertEqual([layer.parallax for layer in layers], [0.12, 0.4, 0.85])
        self.assertEqual(
            [layer.native_size[0] * layer.wraps / settings.duration_seconds for layer in layers],
            [96, 192, 288],
        )
        self.assertEqual(
            [layer.period * layer.wraps / settings.duration_seconds for layer in layers],
            [60, 120, 180],
        )
        for layer in layers:
            self.assertEqual(MODULE.layer_phase(384, layer.period, layer.wraps, 384), 0)
            self.assertEqual(
                MODULE.layer_phase(1, layer.period, layer.wraps, 384),
                layer.period * layer.wraps / 384,
            )

    def test_periodic_fractional_sampling_preserves_coverage(self):
        settings = MODULE.settings_for(manifest(), parameters())
        for alpha in (127, 255):
            tile = Image.new("RGBA", (7, 128), (100, 140, 200, alpha))
            layers = MODULE.prepare_layers(manifest(), {"tile": tile}, settings)
            reference = MODULE.render_frame(layers, settings, 0)
            for frame_index in range(settings.frame_count):
                frame = MODULE.render_frame(layers, settings, frame_index)
                self.assertEqual(MODULE.pixel_difference(frame, reference), 0)

    def test_periodic_endpoint_matches_with_visible_pattern(self):
        settings = MODULE.settings_for(manifest(), parameters())
        tile = Image.new("RGBA", (32, 128), (200, 10, 60, 255))
        for x in range(16):
            for y in range(128):
                tile.putpixel((x, y), (10, 100, 200, 255))
        layers = MODULE.prepare_layers(manifest(), {"tile": tile}, settings)
        first = MODULE.render_frame(layers, settings, 0)
        endpoint = MODULE.render_frame(layers, settings, settings.frame_count)
        moving = MODULE.render_frame(layers, settings, 1)
        self.assertEqual(MODULE.pixel_difference(first, endpoint), 0)
        self.assertGreater(MODULE.pixel_difference(first, moving), 10)

    def test_settings_reject_invalid_and_unbounded_work(self):
        for field, value in (
            ("width", True),
            ("width", 257),
            ("fps", 0),
            ("duration_seconds", 100),
            ("layer_wraps", {}),
            ("layer_wraps", []),
            ("layer_wraps", {"tile": True}),
            ("layer_wraps", {"tile": 0}),
            ("layer_wraps", {"tile": 17}),
            ("layer_wraps", {"tile": 1.0}),
            ("layer_wraps", {"": 1}),
        ):
            params = {**parameters(), field: value}
            with self.assertRaises(ValueError):
                MODULE.settings_for(manifest(), params)
        with self.assertRaises(ValueError):
            MODULE.settings_for(
                manifest(),
                {"width": 1536, "fps": 60, "duration_seconds": 20, "layer_wraps": {"tile": 1}},
            )

    def test_layer_validation(self):
        settings = MODULE.settings_for(manifest(), parameters())
        picture = Image.new("RGBA", (32, 128))
        for invalid in (
            {**manifest(), "layers": []},
            {**manifest(), "layers": [*manifest()["layers"], *manifest()["layers"]]},
            {**manifest(), "layers": [{**manifest()["layers"][0], "parallax": float("nan")}]},
        ):
            with self.assertRaises(ValueError):
                MODULE.prepare_layers(invalid, {"tile": picture}, settings)
        with self.assertRaises(ValueError):
            MODULE.prepare_layers(manifest(), {"different": picture}, settings)
        for wraps in ({"different": 1}, {"tile": 1, "extra": 1}):
            settings = MODULE.settings_for(manifest(), {**parameters(), "layer_wraps": wraps})
            with self.assertRaisesRegex(ValueError, "exactly match"):
                MODULE.prepare_layers(manifest(), {"tile": picture}, settings)

    def test_vertical_alpha_composition_is_not_double_pasted(self):
        scene = manifest()
        scene["layers"][0]["offset_y"] = 32
        settings = MODULE.settings_for(scene, parameters())
        layers = MODULE.prepare_layers(
            scene, {"tile": Image.new("RGBA", (32, 64), (255, 0, 0, 127))}, settings
        )
        frame = MODULE.render_frame(layers, settings, 3)
        self.assertEqual(frame.getpixel((10, 0)), MODULE.BACKGROUND_RGB)
        self.assertEqual(frame.getpixel((10, 127)), MODULE.BACKGROUND_RGB)
        expected = (
            Image.alpha_composite(
                Image.new("RGBA", (1, 1), (*MODULE.BACKGROUND_RGB, 255)),
                Image.new("RGBA", (1, 1), (255, 0, 0, 127)),
            )
            .convert("RGB")
            .getpixel((0, 0))
        )
        for x in range(256):
            self.assertEqual(frame.getpixel((x, 40)), expected)

    @unittest.skipUnless(
        shutil.which("ffmpeg") and shutil.which("ffprobe"),
        "the optional encoded smoke test requires ffmpeg and ffprobe",
    )
    def test_real_encoding_and_portable_report(self):
        with tempfile.TemporaryDirectory(prefix="fx-parallax-preview-") as folder:
            ctx = context(Path(folder))
            result = MODULE.render_preview(ctx)
            report = result["report"].json
            self.assertEqual(set(result), {"animation", "poster", "report"})
            self.assertEqual(result["animation"].kind, "video/mp4")
            self.assertTrue((Path(folder) / "work" / result["animation"].work_path).is_file())
            self.assertFalse((Path(folder) / "work/frames.rgb").exists())
            self.assertEqual(report["frame_count"], 24)
            self.assertEqual(report["endpoint_mean_absolute_pixel_difference"], 0)
            self.assertEqual(report["duration_seconds"], 2)
            self.assertEqual(report["motion_model"], "forward_integer_wraps")
            self.assertTrue(report["retimed_preview"])
            self.assertEqual(report["layer_wraps"], {"tile": 1})
            self.assertEqual(report["layers"][0]["source_parallax"], 0.85)
            self.assertEqual(report["layers"][0]["wrap_count"], 1)
            self.assertEqual(report["layers"][0]["native_pixels_per_second"], 16)
            self.assertEqual(report["layers"][0]["export_pixels_per_second"], 16)
            self.assertNotIn("travel_pixels", report)
            self.assertNotIn(str(Path(folder)), json.dumps(report))
            self.assertEqual(ctx.channel.notifications[-1][1]["fraction"], 1)

    def test_cancellation_cleans_temporary_raw_file(self):
        with tempfile.TemporaryDirectory(prefix="fx-parallax-cancel-") as folder:
            ctx = context(Path(folder))
            ctx.cancelled = True
            with self.assertRaisesRegex(NodeFailure, "cancelled"):
                MODULE.render_preview(ctx)
            self.assertFalse((Path(folder) / "work/frames.rgb").exists())
            self.assertFalse((Path(folder) / "work/out/preview.mp4").exists())


if __name__ == "__main__":
    unittest.main(verbosity=2)
