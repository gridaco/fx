"""The standard bodies (``grida.fx.std``) against the outputs FX's predecessor recorded.

Each body runs with a stand-in ctx: a test double holding exactly the members the bodies use
(``params``, ``inputs``, ``read.image``, ``read.annotations``, ``out.bytes``, ``out.text``,
``out.json``, ``fact``), so these tests hold the bodies alone, apart from the node host. Inputs
come from ``std_fixtures.py``; a picture output is compared by its digest with the predecessor's,
and a JSON output by its value, because FX's engine writes JSON files itself.
"""

from __future__ import annotations

import io
import json
import platform
import subprocess
import sys
import zlib
from pathlib import Path
from typing import Any

import PIL
import pytest
import std_fixtures
from PIL import Image, UnidentifiedImageError
from std_fixtures import INPUT_DIGESTS, sha256

import grida.fx as fx
from grida.fx import NodeFailure
from grida.fx.std import BODIES, body_of, media, pictures

# ------------------------------------------------------------------------------------------------
# The stand-in ctx


class InputFile(fx.InputFile):
    """A file the engine handed the body, as the bodies read one: ``path``, ``kind``, ``name``,
    ``digest``, ``read_bytes()``. It is named like the SDK's class because one recorded error
    names the class (``'InputFile' object is not iterable``)."""

    def __init__(self, path: Path | None, kind: str, name: str, data: bytes | None = None) -> None:
        # The SDK's constructor is not called: this double holds its own members only.
        stored = path.read_bytes() if path is not None else data or b""
        self._path = path
        self._kind = kind
        self._name = name
        self._digest = sha256(stored)

    @property
    def path(self) -> Path | None:  # type: ignore[override]
        return self._path

    @property
    def kind(self) -> str:
        return self._kind

    @property
    def name(self) -> str:
        return self._name

    @property
    def digest(self) -> str:
        return self._digest

    def read_bytes(self) -> bytes:
        assert self._path is not None
        return self._path.read_bytes()


class Made:
    """What ``ctx.out`` returned: bytes with a kind, or a JSON value (kind ``json``)."""

    def __init__(self, kind: str, data: bytes | None = None, value: Any = None) -> None:
        self.kind = kind
        self.data = data
        self.value = value

    @property
    def digest(self) -> str:
        assert self.data is not None
        return sha256(self.data)

    @property
    def text(self) -> str:
        assert self.data is not None
        return self.data.decode("utf-8")


class _Read:
    def __init__(self, ctx: StandIn) -> None:
        self._ctx = ctx

    def _one(self, name: str) -> InputFile:
        value = self._ctx.inputs.get(name)
        if not isinstance(value, fx.InputFile):
            raise NodeFailure(f"input {name} is not one file")
        return value

    def image(self, name: str) -> Any:
        return pictures.read(self._one(name).path)

    def annotations(self, name: str) -> Any:
        return json.loads(self._one(name).read_bytes())


class _Out:
    def bytes(self, data: bytes, kind: str) -> Made:
        return Made(kind, data=bytes(data))

    def text(self, text: str, kind: str = "text/plain") -> Made:
        return Made(kind, data=text.encode("utf-8"))

    def json(self, value: Any) -> Made:
        json.dumps(value, allow_nan=False)  # the engine takes JSON values only
        return Made("json", value=value)


class StandIn:
    """The members of ``Ctx`` the nine bodies use."""

    def __init__(self, *, inputs: dict[str, Any] | None = None, **params: Any) -> None:
        self.inputs = inputs or {}
        self.params = params
        self.facts: dict[str, Any] = {}
        self.read = _Read(self)
        self.out = _Out()
        #: What the body returned (set by :func:`run`).
        self.result: Any = None

    def fact(self, name: str, value: Any) -> None:
        json.dumps(value)
        self.facts[name] = value


#: The defaults the engine fills in from each type's declaration (its catalog).
DEFAULTS: dict[str, dict[str, Any]] = {
    "image.mirror_repeat": {"axis": "x"},
    "image.check_alpha": {"expect": "transparent"},
    "image.crop": {"padding": 0},
    "package": {"manifest": {}},
}


def run(name: str, *, inputs: dict[str, Any] | None = None, **params: Any) -> StandIn:
    """Runs body ``name`` with the engine's defaults under ``params``; returns its ctx, with the
    outputs in ``ctx.result``."""
    ctx = StandIn(inputs=inputs, **{**DEFAULTS.get(name, {}), **params})
    ctx.result = BODIES[name](ctx)
    return ctx


def gnode_json(value: Any) -> bytes:
    """A JSON value as the predecessor wrote it (``ctx.out.json``)."""
    return json.dumps(value, ensure_ascii=False, sort_keys=True, indent=1).encode("utf-8")


def engine_json(data: bytes) -> Any:
    """A JSON file's content as FX's engine hands it to a param: one number type, so a whole
    number arrives as an ``int`` however the file wrote it."""

    def number(value: Any) -> Any:
        if isinstance(value, float) and value.is_integer() and abs(value) < 1e21:
            return int(value)
        if isinstance(value, dict):
            return {key: number(item) for key, item in value.items()}
        if isinstance(value, list):
            return [number(item) for item in value]
        return value

    return number(json.loads(data))


# ------------------------------------------------------------------------------------------------
# Inputs

KINDS = {".png": "image/png", ".jpg": "image/jpeg", ".json": "json", ".md": "text/markdown"}


class Inputs:
    """The synthesized inputs in a folder; an input whose bytes differ from the recorded ones
    skips the test that needs it."""

    def __init__(self, folder: Path) -> None:
        self.paths = std_fixtures.write_all(folder)
        digests = {name: sha256(path.read_bytes()) for name, path in self.paths.items()}
        self.differing = {
            name: digest for name, digest in digests.items() if digest != INPUT_DIGESTS[name]
        }

    def file(self, name: str, *, kind: str | None = None, display: str | None = None) -> InputFile:
        if name in self.differing:
            pytest.skip(
                f"{name} synthesized here has digest {self.differing[name]}, not the recorded "
                f"{INPUT_DIGESTS[name]}: this Python's zlib or this Pillow build writes other "
                "bytes, so the outputs recorded on it cannot be compared"
            )
        path = self.paths[name]
        return InputFile(path, kind or KINDS[path.suffix], display or name)

    def made(self, made: Made, folder: Path, display: str) -> InputFile:
        """An output of one body handed to the next, as the engine stores and hands it on."""
        path = folder / made.digest
        path.write_bytes(made.data or b"")
        return InputFile(path, made.kind, display)


@pytest.fixture(scope="module")
def given(tmp_path_factory: pytest.TempPathFactory) -> Inputs:
    return Inputs(tmp_path_factory.mktemp("inputs"))


def test_the_synthesized_inputs_are_the_recorded_bytes(given: Inputs) -> None:
    if given.differing:
        pytest.skip(
            f"inputs differ from the recorded ones: {sorted(given.differing)} (Python "
            f"{sys.version.split()[0]}, zlib {zlib.ZLIB_RUNTIME_VERSION}, Pillow "
            f"{PIL.__version__}); the goldens were recorded with CPython 3.12, zlib 1.2.12 and "
            "Pillow 12.3.0"
        )
    assert set(given.paths) == set(INPUT_DIGESTS)


# ------------------------------------------------------------------------------------------------
# Registration


def test_bodies_are_the_nine_by_name() -> None:
    assert BODIES == {
        "image.mirror_repeat": media.mirror_repeat,
        "image.check_alpha": media.check_alpha,
        "image.check_size": media.check_size,
        "image.resize": media.resize,
        "image.crop": media.crop,
        "image.pad": media.pad,
        "json.merge": media.json_merge,
        "files.copy": media.files_copy,
        "package": media.package,
    }
    for name, body in BODIES.items():
        assert body_of(f"fx/{name}@1") is body


@pytest.mark.parametrize(
    "builtin",
    [
        "fx/image.resize@2",
        "fx/image.resize@01",
        "fx/image.resize@1.1",
        "fx/image.resize@1\n",
        "fx/image.resize",
        "gnode/image.resize@1",
        "image.resize",
        "fx/image.generate@1",
        "fx/select@1",
        "fx/image.key@1",
        "",
    ],
)
def test_no_body_for_other_names_and_majors(builtin: str) -> None:
    assert body_of(builtin) is None


def test_no_body_for_a_value_that_is_not_text() -> None:
    assert body_of(None) is None  # type: ignore[arg-type]


# ------------------------------------------------------------------------------------------------
# Pictures


def test_read_is_a_loaded_copy_of_the_first_frame(given: Inputs) -> None:
    picture = pictures.read(given.file("p_trns.png").path)
    assert picture.mode == "P"
    assert picture.size == (16, 8)
    assert picture.info["transparency"] == 0
    assert picture.getpixel((1, 0)) == 1  # read after the file was closed


def test_bodies_that_touch_no_picture_need_no_pillow(tmp_path: Path) -> None:
    script = (
        "import json, sys\n"
        "sys.modules['PIL'] = None\n"
        "from grida.fx.std import body_of, pictures\n"
        "assert body_of('fx/json.merge@1') is not None\n"
        "try:\n"
        "    pictures.png(object())\n"
        "except ImportError:\n"
        "    print('pillow is imported when a picture is first needed')\n"
    )
    done = subprocess.run(
        [sys.executable, "-c", script], capture_output=True, text=True, timeout=60, cwd=tmp_path
    )
    assert done.returncode == 0, done.stderr
    assert done.stdout == "pillow is imported when a picture is first needed\n"


def test_png_takes_only_a_pil_image() -> None:
    with pytest.raises(TypeError) as raised:
        pictures.png("a picture")
    assert str(raised.value) == "ctx.out.png takes a PIL image, not str"


def test_png_writes_what_the_bodies_write(given: Inputs) -> None:
    picture = pictures.read(given.file("rgba_icc.png").path)
    assert pictures.png(picture) == media._png(picture)
    assert sha256(pictures.png(picture)) == INPUT_DIGESTS["rgba_icc.png"]


# ------------------------------------------------------------------------------------------------
# Picture outputs: the recorded digests

MIRROR_X = "584b0b17da6d071566de76b61062851a9538e4771fddedd4e44028e74891bea7"
MIRROR_Y = "a19b87ca230e4b539a1a27f6089be60eb83abec5dbbc14d714b74f04c351d36f"
MIRROR_XY = "cdee893f8fc3b9456035d913150f159b841b0fe1bc60e9f3d28ce442caa1da5c"
RESIZE_16 = "37deea4bf3453beb74c977e42f9bb5cf885d2fc93f6e337bb26db3d0745dad9f"

#: The recorded digest of each picture output (some reproduce their input byte for byte). They
#: hold where they were recorded, on macOS arm64 (:data:`RECORDED_ON`): with the same inputs and
#: the same Pillow, Pillow's Linux x86-64 build writes other PNG bytes for some of these pictures.
RECORDED = {
    "mirror_x": MIRROR_X,
    "mirror_y": MIRROR_Y,
    "mirror_xy": MIRROR_XY,
    "mirror_yx": MIRROR_XY,
    "mirror_list": MIRROR_XY,
    "mirror_icc": MIRROR_X,
    "mirror_z": INPUT_DIGESTS["rgba_icc.png"],
    "mirror_pal": "b0daa0c786a3fe6bd45a6eb9de16c58ccbf3b5dc137f6f331bd74be66b3cf85c",
    "mirror_wide_ok": "3a3b733315fe39dd3b97181d2617d739f6973980b15f6d1fe4a4bb54aca25bbb",
    "resize_long": RESIZE_16,
    "resize_wh": "eb0246643edba4d75390bf7ea2cf8cb5443149605f7a54c672ccbed5b2e886fa",
    "resize_same": INPUT_DIGESTS["rgba_37x23.png"],
    "resize_wins": RESIZE_16,
    "resize_icc": "4715e0e3303f7c78b749a53d306b5071131b3ccf156316049a880332aded8725",
    "resize_tie": "a1a0fdc69eec3656da1069c70ce1002fed007dcf05b000281f9410c2ae9ad931",
    "resize_tie6": "8a52ea52716860452dde4264873b65fde6553c5ef8a3f515f9dc9e690a4818dd",
    "resize_frac": "e557187759233e4c3bd197a2a0ae35a13242a0f95df2ae2af3f601b1040e3f34",
    "resize_zero": "ac5d59c07e56ce5c075672d90e4c439fba9a4f8f200f9d1a8a43ccbde81ed9c5",
    "resize_neg": "f2496332ed428d7c999b79e263f289be0e7b21a2fccbbe694d0709e2b4b43f13",
    "resize_g16": "38fb43fbd4b078237a94650903231de41f9ac8ed0c2550b3581cd41b6e9ff501",
    "crop_box": "aab6f1acbc2674831fbcd418283238490c0ea467a79bdf8efdf248439427b964",
    "crop_pad": INPUT_DIGESTS["rgba_37x23.png"],
    "crop_half": "9f1b6a54e4e965f0a50253a5b490b8cf8a0eb9ca8a4e900e3cfa7083442d4fd3",
    "crop_negpad": "45d134e524678a77ce9bb50e3ce29c9e743361e9499222cac071478457ba77f4",
    "crop_out": INPUT_DIGESTS["rgba_37x23.png"],
    "pad_ok": "1df73a97baa67d429d15905d673d81b1362480ae5487adc9e27369bf44e0408c",
    "pad_odd": "bf1fe9c4ec7af13c5cdf4413b94962ba30001319908527b52210739d4212e251",
    "pad_icc": INPUT_DIGESTS["rgba_37x23.png"],
}

#: Where :data:`RECORDED` holds, as ``(sys.platform, platform.machine())``.
RECORDED_ON = ("darwin", "arm64")

#: The digest of each picture output's RGBA pixels, which hold on every platform.
PIXELS = {
    "mirror_x": "1c910ab8fc7d11d4bd6880b69b3ffab367a4746712eaba1e5270d421b55aa858",
    "mirror_y": "93256bbdaa1e305f0d46339121a0927d8e71e2ddec0a2ef4c4c5651bb8fd36af",
    "mirror_xy": "d7d13934fc637bde832a0a085de1c5741344f4f79d8afd2a6f1a7d821c0e4ce7",
    "mirror_yx": "d7d13934fc637bde832a0a085de1c5741344f4f79d8afd2a6f1a7d821c0e4ce7",
    "mirror_list": "d7d13934fc637bde832a0a085de1c5741344f4f79d8afd2a6f1a7d821c0e4ce7",
    "mirror_icc": "1c910ab8fc7d11d4bd6880b69b3ffab367a4746712eaba1e5270d421b55aa858",
    "mirror_z": "4532fccadccbbdf7a083b89fdc11000642f6f17325d8351ac039fcc3546e1f75",
    "mirror_pal": "5bf3e46a75ba37a66a480ce9330be965c76aedf99679b28cf38af48116636e57",
    "mirror_wide_ok": "119172c65e828369fc554b68d4f312ee98ac77102f5fd13215ad03ac6f24c6ce",
    "resize_long": "5e577f1d61c841f20a8f3301febadcd7943d5b7e4deb5363cfe5b244ae8f4f64",
    "resize_wh": "9e403e09e2c901dba803f0fdcd41d9cbbb213b8863a8bdcfabd9ba098b695f5c",
    "resize_same": "4532fccadccbbdf7a083b89fdc11000642f6f17325d8351ac039fcc3546e1f75",
    "resize_wins": "5e577f1d61c841f20a8f3301febadcd7943d5b7e4deb5363cfe5b244ae8f4f64",
    "resize_icc": "ee2458eb3eb78add5360d3a2fde00ac9c087d0443111c2b54b10db39a67da959",
    "resize_tie": "d333233175203f086ecb923af3a188f07d70c77d797467c697b7a0eaea08e9b5",
    "resize_tie6": "f199624d6293e1f7dc69ceadf4bcc1dcdf12b1b4100b0e4ffeed9b06a99fa16b",
    "resize_frac": "054f7c32b9624c9e27f5b4d676ccb38c1053b70a974d6c7d96356ff78ac51e07",
    "resize_zero": "1f14bd8e308ebe9dc78751dce7a329816030429b5a647e0299bf7f1060422d31",
    "resize_neg": "a5c3a910013439a359b01a20f83c6b4f73d105303ef80d7af26f5fde5005950a",
    "resize_g16": "3d6876a0146de8576eb2395a858de1213d1b92c65b779df3a331cfd5a4584546",
    "crop_box": "45b063f48761946c8e1e17abc91946b02d47d9f395006c35a8649fc7b7a57987",
    "crop_pad": "4532fccadccbbdf7a083b89fdc11000642f6f17325d8351ac039fcc3546e1f75",
    "crop_half": "0e697b7d6a9cfacce383279333f9313bdba150292dd1b79903c6161cef117853",
    "crop_negpad": "b2691ed692bc3f874b7ddd5f4769aaf54f85bf2309aee8f6a79bdc8286912805",
    "crop_out": "4532fccadccbbdf7a083b89fdc11000642f6f17325d8351ac039fcc3546e1f75",
    "pad_ok": "7297c5d51785d023bed0d5c2e716ddf9c257f4c1026fad06d3b2d2b4c5bc65b8",
    "pad_odd": "db6687abc9a46afae8b68eb05394acc74e6dfa4b41412bd9b0ea80f56eb0b536",
    "pad_icc": "4532fccadccbbdf7a083b89fdc11000642f6f17325d8351ac039fcc3546e1f75",
}

PICTURES = [
    # (case, body, input, params, (width, height) of the output)
    ("mirror_x", "image.mirror_repeat", "rgba_37x23.png", {}, (74, 23)),
    ("mirror_y", "image.mirror_repeat", "rgba_37x23.png", {"axis": "y"}, (37, 46)),
    ("mirror_xy", "image.mirror_repeat", "rgba_37x23.png", {"axis": "xy"}, (74, 46)),
    ("mirror_yx", "image.mirror_repeat", "rgba_37x23.png", {"axis": "yx"}, (74, 46)),
    ("mirror_list", "image.mirror_repeat", "rgba_37x23.png", {"axis": ["x", "y"]}, (74, 46)),
    ("mirror_icc", "image.mirror_repeat", "rgba_icc.png", {"axis": "x"}, (74, 23)),
    ("mirror_z", "image.mirror_repeat", "rgba_icc.png", {"axis": "z"}, (37, 23)),
    ("mirror_pal", "image.mirror_repeat", "p_trns.png", {"axis": "x"}, (32, 8)),
    ("mirror_wide_ok", "image.mirror_repeat", "wide_8192.png", {}, (16384, 2)),
    ("resize_long", "image.resize", "rgba_37x23.png", {"longest_side": 16}, (16, 10)),
    ("resize_wh", "image.resize", "rgba_37x23.png", {"width": 10, "height": 50}, (10, 50)),
    ("resize_same", "image.resize", "rgba_37x23.png", {"longest_side": 37}, (37, 23)),
    (
        "resize_wins",
        "image.resize",
        "rgba_37x23.png",
        {"longest_side": 16, "width": 10, "height": 50},
        (16, 10),
    ),
    ("resize_icc", "image.resize", "rgba_icc.png", {"longest_side": 74}, (74, 46)),
    ("resize_tie", "image.resize", "rgb_40x30.jpg", {"longest_side": 5}, (5, 4)),
    ("resize_tie6", "image.resize", "rgb_40x30.jpg", {"longest_side": 6}, (6, 4)),
    ("resize_frac", "image.resize", "rgb_40x30.jpg", {"longest_side": 8.5}, (8, 6)),
    ("resize_zero", "image.resize", "rgb_40x30.jpg", {"longest_side": 0}, (1, 1)),
    ("resize_neg", "image.resize", "rgba_37x23.png", {"longest_side": -4}, (1, 1)),
    ("resize_g16", "image.resize", "gray16.png", {"longest_side": 8}, (8, 8)),
    ("crop_box", "image.crop", "rgba_37x23.png", {"box": [0.1, 0.2, 0.6, 0.9]}, (18, 16)),
    (
        "crop_pad",
        "image.crop",
        "rgba_37x23.png",
        {"box": [0.25, 0.25, 0.75, 0.75], "padding": 0.5},
        (37, 23),
    ),
    ("crop_half", "image.crop", "rgb_40x30.jpg", {"box": [0.0125, 0, 0.5125, 0.5]}, (20, 15)),
    (
        "crop_negpad",
        "image.crop",
        "rgba_37x23.png",
        {"box": [0, 0, 1, 1], "padding": -0.25},
        (19, 11),
    ),
    ("crop_out", "image.crop", "rgba_37x23.png", {"box": [-0.5, -0.5, 2, 2]}, (37, 23)),
    ("pad_ok", "image.pad", "rgba_37x23.png", {"width": 40, "height": 30}, (40, 30)),
    ("pad_odd", "image.pad", "rgba_37x23.png", {"width": 40, "height": 26}, (40, 26)),
    ("pad_icc", "image.pad", "rgba_icc.png", {"width": 37, "height": 23}, (37, 23)),
]

BOX_PX = {
    "crop_box": [4, 5, 22, 21],
    "crop_pad": [0, 0, 37, 23],
    "crop_half": [0, 0, 20, 15],
    "crop_negpad": [9, 6, 28, 17],
    "crop_out": [0, 0, 37, 23],
}


@pytest.mark.parametrize(
    ("case", "body", "source", "params", "size"), PICTURES, ids=[case[0] for case in PICTURES]
)
def test_a_picture_output_is_the_recorded_bytes(
    given: Inputs,
    case: str,
    body: str,
    source: str,
    params: dict[str, Any],
    size: tuple[int, int],
) -> None:
    ctx = run(body, inputs={"image": given.file(source)}, **params)
    image = ctx.result["image"]
    assert image.kind == "image/png"
    with Image.open(_bytes_io(image.data)) as written:
        assert written.size == size
        assert written.mode == "RGBA"
        assert sha256(written.tobytes()) == PIXELS[case]
    if (sys.platform, platform.machine()) == RECORDED_ON:
        assert image.digest == RECORDED[case]
    if body == "image.crop":
        assert ctx.facts == {"box_px": BOX_PX[case]}
    else:
        assert ctx.facts == {}


def _bytes_io(data: bytes | None) -> io.BytesIO:
    return io.BytesIO(data or b"")


def test_a_kept_icc_profile_and_a_dropped_one(given: Inputs) -> None:
    resized = run("image.resize", inputs={"image": given.file("rgba_icc.png")}, longest_side=74)
    with Image.open(_bytes_io(resized.result["image"].data)) as picture:
        assert "icc_profile" in picture.info
    padded = run("image.pad", inputs={"image": given.file("rgba_icc.png")}, width=40, height=30)
    with Image.open(_bytes_io(padded.result["image"].data)) as picture:
        assert "icc_profile" not in picture.info


def test_resize_rounds_half_to_even_and_keeps_a_pixel(given: Inputs) -> None:
    neg = run("image.resize", inputs={"image": given.file("rgba_37x23.png")}, longest_side=-4)
    with Image.open(_bytes_io(neg.result["image"].data)) as picture:
        assert picture.getpixel((0, 0)) == (142, 127, 113, 132)
    g16 = run("image.resize", inputs={"image": given.file("gray16.png")}, longest_side=8)
    with Image.open(_bytes_io(g16.result["image"].data)) as picture:
        assert picture.getextrema() == ((255, 255), (255, 255), (255, 255), (255, 255))


def test_crop_takes_the_first_box_of_a_region(given: Inputs) -> None:
    ctx = run(
        "image.crop",
        inputs={"image": given.file("rgba_icc.png"), "region": given.file("region.json")},
    )
    assert ctx.facts == {"box_px": [9, 6, 28, 17]}
    assert (
        ctx.result["image"].digest
        == "89430d1bdd658dad3b00ac322efa3aaf631b067143e952710997f3a8eb70bc88"
    )


def test_a_box_param_wins_over_the_region(given: Inputs) -> None:
    ctx = run(
        "image.crop",
        inputs={"image": given.file("rgba_37x23.png"), "region": given.file("region.json")},
        box=[0.1, 0.2, 0.6, 0.9],
    )
    assert ctx.facts == {"box_px": [4, 5, 22, 21]}


def test_an_absent_region_is_no_region(given: Inputs) -> None:
    with pytest.raises(NodeFailure) as raised:
        run("image.crop", inputs={"image": given.file("rgba_37x23.png"), "region": None})
    assert str(raised.value) == "crop takes box: [x0, y0, x1, y1], or a region with a box mark"


# ------------------------------------------------------------------------------------------------
# Judges: the recorded facts, in the order reported

JUDGES = [
    (
        "alpha_t",
        "image.check_alpha",
        "rgba_37x23.png",
        {},
        {"alpha_min": 0, "alpha_max": 255, "verdict": "accept"},
    ),
    (
        "alpha_o",
        "image.check_alpha",
        "rgba_37x23.png",
        {"expect": "opaque"},
        {"alpha_min": 0, "alpha_max": 255, "verdict": "reject"},
    ),
    (
        "alpha_jpg_t",
        "image.check_alpha",
        "rgb_40x30.jpg",
        {},
        {"alpha_min": 255, "alpha_max": 255, "verdict": "reject"},
    ),
    (
        "alpha_jpg_o",
        "image.check_alpha",
        "rgb_40x30.jpg",
        {"expect": "opaque"},
        {"alpha_min": 255, "alpha_max": 255, "verdict": "accept"},
    ),
    (
        "alpha_pal",
        "image.check_alpha",
        "p_trns.png",
        {"expect": "whatever"},
        {"alpha_min": 0, "alpha_max": 255, "verdict": "reject"},
    ),
    (
        "alpha_g16",
        "image.check_alpha",
        "gray16.png",
        {"expect": "opaque"},
        {"alpha_min": 255, "alpha_max": 255, "verdict": "accept"},
    ),
    (
        "size_ok",
        "image.check_size",
        "rgba_37x23.png",
        {"width": 37, "height": 23},
        {"width": 37, "height": 23, "verdict": "accept"},
    ),
    (
        "size_w",
        "image.check_size",
        "rgba_37x23.png",
        {"width": 37.0},
        {"width": 37, "height": 23, "verdict": "accept"},
    ),
    (
        "size_bad",
        "image.check_size",
        "rgba_37x23.png",
        {"height": 24},
        {"width": 37, "height": 23, "verdict": "reject"},
    ),
    (
        "size_none",
        "image.check_size",
        "rgba_37x23.png",
        {},
        {"width": 37, "height": 23, "verdict": "accept"},
    ),
]


@pytest.mark.parametrize(
    ("case", "body", "source", "params", "facts"), JUDGES, ids=[case[0] for case in JUDGES]
)
def test_a_judge_reports_the_recorded_facts(
    given: Inputs,
    case: str,
    body: str,
    source: str,
    params: dict[str, Any],
    facts: dict[str, Any],
) -> None:
    ctx = run(body, inputs={"image": given.file(source)}, **params)
    assert ctx.result == {}
    assert list(ctx.facts.items()) == list(facts.items())


# ------------------------------------------------------------------------------------------------
# Errors: the recorded messages

FAILURES = [
    (
        "mirror_wide",
        "image.mirror_repeat",
        "wide_8193.png",
        {},
        "mirroring 8193 px on x exceeds 16384 px",
    ),
    (
        "mirror_tall",
        "image.mirror_repeat",
        "tall_8193.png",
        {"axis": "xy"},
        "mirroring 8193 px on y exceeds 16384 px",
    ),
    (
        "resize_w_only",
        "image.resize",
        "rgba_37x23.png",
        {"width": 10},
        "resize takes longest_side, or width and height",
    ),
    (
        "resize_none",
        "image.resize",
        "rgba_37x23.png",
        {},
        "resize takes longest_side, or width and height",
    ),
    (
        "crop_nobox",
        "image.crop",
        "rgba_37x23.png",
        {},
        "crop takes box: [x0, y0, x1, y1], or a region with a box mark",
    ),
    (
        "crop_three",
        "image.crop",
        "rgba_37x23.png",
        {"box": [0, 0, 1]},
        "crop takes box: [x0, y0, x1, y1], or a region with a box mark",
    ),
    (
        "crop_empty",
        "image.crop",
        "rgba_37x23.png",
        {"box": [0.5, 0.5, 0.5, 1.0]},
        "the box [0.5, 0.5, 0.5, 1.0] is empty on a 37x23 picture",
    ),
    (
        "crop_empty_fx",
        "image.crop",
        "rgba_37x23.png",
        {"box": [0.5, 0.5, 0.5, 1]},
        "the box [0.5, 0.5, 0.5, 1] is empty on a 37x23 picture",
    ),
    (
        "crop_rev",
        "image.crop",
        "rgba_37x23.png",
        {"box": [0.9, 0, 0.1, 1]},
        "the box [0.9, 0, 0.1, 1] is empty on a 37x23 picture",
    ),
    (
        "pad_small",
        "image.pad",
        "rgba_37x23.png",
        {"width": 36, "height": 30},
        "a 37x23 picture does not fit 36x30",
    ),
]


@pytest.mark.parametrize(
    ("case", "body", "source", "params", "message"), FAILURES, ids=[case[0] for case in FAILURES]
)
def test_a_body_fails_with_the_recorded_message(
    given: Inputs, case: str, body: str, source: str, params: dict[str, Any], message: str
) -> None:
    ctx = StandIn(inputs={"image": given.file(source)}, **{**DEFAULTS.get(body, {}), **params})
    with pytest.raises(NodeFailure) as raised:
        BODIES[body](ctx)
    assert str(raised.value) == message
    assert ctx.facts == {}


def test_crop_fails_on_a_region_without_a_box(given: Inputs) -> None:
    with pytest.raises(NodeFailure) as raised:
        run(
            "image.crop",
            inputs={"image": given.file("rgba_37x23.png"), "region": given.file("noshape.json")},
        )
    assert str(raised.value) == "the region has no box to crop to"


def test_unexpected_errors_are_python_errors(given: Inputs) -> None:
    image = {"image": given.file("rgba_37x23.png")}
    with pytest.raises(ValueError) as raised:
        run("image.resize", inputs=image, width=0, height=5)
    assert str(raised.value) == "height and width must be > 0"
    with pytest.raises(ValueError) as raised:
        run("image.crop", inputs=image, box="abcd")
    assert str(raised.value) == "could not convert string to float: 'a'"
    with pytest.raises(UnidentifiedImageError) as unreadable:
        run("image.check_alpha", inputs={"image": given.file("not_an_image.png")})
    assert str(unreadable.value).startswith("cannot identify image file ")


# ------------------------------------------------------------------------------------------------
# json.merge and files.copy

MERGED_BY_GNODE = "d7288468b8e03d08fff40f91d550e9db286cbc3fe6bb2e0f1ecba7346fd4354e"


def test_merge_is_shallow_and_a_later_key_wins(given: Inputs) -> None:
    ctx = run("json.merge", inputs={"documents": [given.file("a.json"), given.file("b.json")]})
    merged = ctx.result["json"]
    assert merged.kind == "json"
    assert merged.value == {
        "b": 1,
        "a": "replaced",
        "z": "é",
        "n": 1e16,
        "s": 1e-05,
        "c": None,
        "d": True,
    }
    assert list(merged.value) == ["b", "a", "z", "n", "s", "c", "d"]
    # The same value, to the number type: the predecessor's writer gives its recorded bytes.
    assert sha256(gnode_json(merged.value)) == MERGED_BY_GNODE


def test_merge_of_nothing_is_an_empty_object() -> None:
    ctx = run("json.merge", inputs={"documents": []})
    assert ctx.result["json"].value == {}
    assert (
        sha256(gnode_json(ctx.result["json"].value))
        == "44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a"
    )


def test_merge_failures(given: Inputs) -> None:
    with pytest.raises(NodeFailure) as raised:
        run("json.merge", inputs={"documents": [given.file("a.json"), given.file("array.json")]})
    assert str(raised.value) == "json.merge merges objects"
    with pytest.raises(UnicodeDecodeError) as undecodable:
        run("json.merge", inputs={"documents": [given.file("rgba_37x23.png")]})
    assert str(undecodable.value) == (
        "'utf-8' codec can't decode byte 0x89 in position 0: invalid start byte"
    )
    with pytest.raises(TypeError) as single:
        run("json.merge", inputs={"documents": given.file("a.json")})
    assert str(single.value) == "'InputFile' object is not iterable"


@pytest.mark.parametrize(
    ("source", "kind"), [("rgba_icc.png", "image/png"), ("a.json", "json"), ("a.json", "file")]
)
def test_copy_keeps_the_bytes_and_the_kind(given: Inputs, source: str, kind: str) -> None:
    ctx = run("files.copy", inputs={"file": given.file(source, kind=kind)})
    copied = ctx.result["file"]
    assert copied.kind == kind
    assert copied.digest == INPUT_DIGESTS[source]


# ------------------------------------------------------------------------------------------------
# package

MANIFEST_BY_GNODE = "ad9d39c92861a72b796f62e5e3809c99ecdcf5cad1a25c0e93bdb87dea1f9910"
MIRROR_X = "584b0b17da6d071566de76b61062851a9538e4771fddedd4e44028e74891bea7"
MIRROR_Y = "a19b87ca230e4b539a1a27f6089be60eb83abec5dbbc14d714b74f04c351d36f"


def _probe_package(given: Inputs, folder: Path, content: Any) -> StandIn:
    """The predecessor's probe step ``pkg``: ``content`` reads a JSON file as a param would."""
    rgba = given.file("rgba_37x23.png")
    jpg = given.file("rgb_40x30.jpg")
    a = given.file("a.json")
    mirror_x = run("image.mirror_repeat", inputs={"image": rgba}).result["image"]
    mirror_y = run("image.mirror_repeat", inputs={"image": rgba}, axis="y").result["image"]
    merged = run("json.merge", inputs={"documents": [a, given.file("b.json")]}).result["json"]
    x = given.made(mirror_x, folder, "mirror_x/image")
    y = given.made(mirror_y, folder, "mirror_y/image")
    files = {
        "a.json": content(a.read_bytes()),
        "merged.json": content(json.dumps(merged.value).encode()),
        "notes.md": given.file("notes.md").read_bytes().decode("utf-8"),
        "notes.txt": "plain words",
        "data.yaml": "a string",
        "facts.json": {"width": 37, "height": 23, "verdict": "accept"},
        "n.json": 3,
        "img/mirror.png": x,
        "img/in.png": rgba,
        "skip.png": None,
        "keyed/{key}.png": {"ada": x, "bo": jpg, "cy": None},
    }
    manifest = {
        "title": "Probe",
        "image": y,
        "input": jpg,
        "doc": content(a.read_bytes()),
        "nested": {"list": [rgba, 1, 1.5]},
    }
    return run("package", files=files, manifest=manifest)


def test_package_lays_out_the_probe_step(given: Inputs, tmp_path: Path) -> None:
    ctx = _probe_package(given, tmp_path, engine_json)
    files = ctx.result["files"]
    assert list(files) == [
        "a.json",
        "merged.json",
        "notes.md",
        "notes.txt",
        "data.yaml",
        "facts.json",
        "n.json",
        "img/mirror.png",
        "img/in.png",
        "keyed/ada.png",
        "keyed/bo.png",
    ]
    doc = {"b": 1, "a": {"x": 1, "y": [1, 2]}, "z": "é", "n": 10**16, "s": 1e-05}
    assert (files["a.json"].kind, files["a.json"].value) == ("json", doc)
    assert files["merged.json"].value == {
        "b": 1,
        "a": "replaced",
        "z": "é",
        "n": 10**16,
        "s": 1e-05,
        "c": None,
        "d": True,
    }
    assert (files["notes.md"].kind, files["notes.md"].text) == (
        "text/markdown",
        "line one\r\nline two\n",
    )
    assert (files["notes.txt"].kind, files["notes.txt"].text) == ("text/plain", "plain words")
    assert (files["data.yaml"].kind, files["data.yaml"].text) == ("text/plain", "a string")
    assert files["facts.json"].value == {"width": 37, "height": 23, "verdict": "accept"}
    assert (files["n.json"].kind, files["n.json"].value) == ("json", 3)
    for path, digest, kind in [
        ("img/mirror.png", MIRROR_X, "image/png"),
        ("img/in.png", INPUT_DIGESTS["rgba_37x23.png"], "image/png"),
        ("keyed/ada.png", MIRROR_X, "image/png"),
        ("keyed/bo.png", INPUT_DIGESTS["rgb_40x30.jpg"], "image/jpeg"),
    ]:
        assert (files[path].kind, files[path].digest) == (kind, digest), path
    manifest = ctx.result["manifest"]
    assert manifest.kind == "json"
    assert manifest.value == {
        "files": [
            "a.json",
            "data.yaml",
            "facts.json",
            "img/in.png",
            "img/mirror.png",
            "keyed/ada.png",
            "keyed/bo.png",
            "merged.json",
            "n.json",
            "notes.md",
            "notes.txt",
        ],
        "title": "Probe",
        "image": {"digest": MIRROR_Y, "kind": "image/png", "name": "mirror_y/image"},
        "input": {
            "digest": INPUT_DIGESTS["rgb_40x30.jpg"],
            "kind": "image/jpeg",
            "name": "rgb_40x30.jpg",
        },
        "doc": doc,
        "nested": {
            "list": [
                {
                    "digest": INPUT_DIGESTS["rgba_37x23.png"],
                    "kind": "image/png",
                    "name": "rgba_37x23.png",
                },
                1,
                1.5,
            ]
        },
    }


def test_package_values_are_the_predecessors_to_the_byte(given: Inputs, tmp_path: Path) -> None:
    """Given the values the predecessor gave it, every JSON file of the package is the value its
    writer recorded: the same keys, order of files, and number types."""
    ctx = _probe_package(given, tmp_path, json.loads)
    files = ctx.result["files"]
    recorded = {
        "a.json": "e2a48138d2c34337741c17ba494109c881042b241796869d15037c11771167d0",
        "facts.json": "2213fee10c29d9df9fecd4fc14af922b90a397b7af68113266d612122202c128",
        "n.json": "4e07408562bedb8b60ce05c1decfe3ad16b72230967de01f640b7e4729b49fce",
    }
    for path, digest in recorded.items():
        assert sha256(gnode_json(files[path].value)) == digest, path
    assert sha256(gnode_json(ctx.result["manifest"].value)) == MANIFEST_BY_GNODE


@pytest.mark.parametrize("destination", ["../x.json", "a/../x.json", "/x.json", "a b.json"])
def test_package_refuses_a_path_outside_it(destination: str) -> None:
    with pytest.raises(NodeFailure) as raised:
        run("package", files={destination: 1})
    assert str(raised.value) == f"'{destination}' is not a relative path inside the package"


@pytest.mark.parametrize("destination", ["a//b.json", "a/", "", "é.json", "a/.."])
def test_package_refuses_what_its_pattern_does_not_match(destination: str) -> None:
    with pytest.raises(NodeFailure) as raised:
        run("package", files={destination: 1})
    assert str(raised.value) == f"{destination!r} is not a relative path inside the package"


def test_package_accepts_dot_segments_braces_and_a_final_newline() -> None:
    ctx = run("package", files={"./x.json": 1, "a/./b.txt": "t", "{x}.json": 2, "end.json\n": 3})
    files = ctx.result["files"]
    assert list(files) == ["./x.json", "a/./b.txt", "{x}.json", "end.json\n"]
    assert (files["a/./b.txt"].kind, files["a/./b.txt"].text) == ("text/plain", "t")
    assert files["{x}.json"].value == 2


def test_package_keys_fill_every_key_placeholder_and_manifest_entries_win() -> None:
    ctx = run("package", files={"{key}/{key}.json": {"a": 1}}, manifest={"files": "overridden"})
    assert list(ctx.result["files"]) == ["a/a.json"]
    assert ctx.result["manifest"].value == {"files": "overridden"}


def test_package_key_destinations_take_a_keyed_collection() -> None:
    with pytest.raises(NodeFailure) as raised:
        run("package", files={"x/{key}.json": [1, 2]})
    assert str(raised.value) == "x/{key}.json names {key}, so it takes a keyed collection"


def test_package_of_nothing() -> None:
    ctx = run("package", files={})
    assert ctx.result["files"] == {}
    assert ctx.result["manifest"].value == {"files": []}
    assert (
        sha256(gnode_json(ctx.result["manifest"].value))
        == "5137627d8e4cfa47bad0b35eb03c9d22c60cf669b82339af7a56cb5377670286"
    )


def test_package_a_later_destination_replaces_an_earlier_one_in_place() -> None:
    ctx = run("package", files={"k/a.json": 1, "k/{key}.json": {"a": 2, "b": 3}})
    files = ctx.result["files"]
    assert list(files) == ["k/a.json", "k/b.json"]
    assert (files["k/a.json"].value, files["k/b.json"].value) == (2, 3)


def test_package_text_kinds_follow_the_destination() -> None:
    ctx = run("package", files={"a.md": "x", "b.txt": "y", "c.yaml": "z", "d": "w"})
    kinds = {path: made.kind for path, made in ctx.result["files"].items()}
    assert kinds == {
        "a.md": "text/markdown",
        "b.txt": "text/plain",
        "c.yaml": "text/plain",
        "d": "text/plain",
    }


def test_package_a_file_without_bytes_fails() -> None:
    nowhere = InputFile(None, "image/png", "draw/image", data=b"x")
    with pytest.raises(NodeFailure) as raised:
        run("package", files={"x.png": nowhere})
    assert str(raised.value) == "x.png: draw/image has no bytes"


def test_package_manifest_must_be_a_mapping() -> None:
    with pytest.raises(TypeError):
        run("package", files={}, manifest=None)
