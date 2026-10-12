"""Draw original software-demo footage for the example, using no network or provider.

Forma is an invented product. This scripted interaction is an authored recording fixture,
not a recording of a shipping application's behavior. Supply your own recording to the utility.
"""

from __future__ import annotations

import argparse
import json
import math
import shutil
import subprocess
from pathlib import Path

from PIL import Image, ImageDraw

from nodes.composition import Type, ease, smooth

ROOT = Path(__file__).parent
SIZE = (1920, 1080)
FPS = 30
SECONDS = 12


def rounded(draw, box, color, radius=16, outline=None, width=1):
    draw.rounded_rectangle(box, radius=radius, fill=color, outline=outline, width=width)


class Demo:
    def __init__(self, variant: str):
        self.variant = variant
        self.type = Type(ROOT / "fonts/Manrope.ttf")
        self.bg = "#f2f3f0"
        self.ink = "#18251e"
        self.accent = "#caff65" if variant == "garden" else "#bbb4ff"
        self.prompt = (
            "Design a landing page\nfor an urban garden."
            if variant == "garden"
            else "Design a bold poster\nfor a night exhibition."
        )
        self.followup = "Make it warmer." if variant == "garden" else "Try a violet palette."
        self.initial_page = self.page(False)
        self.refined_page = self.page(True)

    def text(self, draw, xy, text, size=24, fill=None, weight=500, anchor=None):
        draw.text(
            xy,
            text,
            font=self.type.font(size, weight),
            fill=fill or self.ink,
            anchor=anchor,
            spacing=7,
        )

    def page(self, refined: bool) -> Image.Image:
        if self.variant == "poster":
            return self.poster_page(refined)
        image = Image.new("RGB", (1320, 820), "#f5eddc" if refined else "#f2f4df")
        draw = ImageDraw.Draw(image)
        self.text(draw, (78, 48), "HARVEST", 26, "#204b2e", 800)
        self.text(draw, (990, 53), "Our story   Visit", 20, "#204b2e", 500)
        draw.line((78, 111, 1242, 111), fill="#cecfb7", width=1)
        self.text(draw, (78, 165), "A LITTLE MORE GREEN", 16, "#49674c", 750)
        self.text(draw, (68, 208), "Room to\ngrow.", 108, "#244d32", 650)
        self.text(
            draw,
            (78, 498),
            "A city garden. A slower moment.\nA place to make something good.",
            25,
            "#49674c",
            500,
        )
        rounded(draw, (78, 609, 319, 670), "#244d32", 30)
        self.text(draw, (109, 625), "Find your green", 23, "#f8f5df", 650)
        # Authored geometric botanical artwork: no external image input.
        draw.ellipse((758, 145, 1218, 605), fill="#ecb189" if refined else "#d8e49b")
        draw.ellipse((793, 178, 1186, 571), outline="#ba7958" if refined else "#bdcc7d", width=2)
        draw.line((984, 534, 981, 265), fill="#315d37", width=13)
        for box, fill in [
            ((838, 279, 986, 378), "#517847"),
            ((983, 237, 1128, 339), "#315d37"),
            ((846, 384, 989, 466), "#6e914e"),
            ((981, 356, 1125, 442), "#3d673d"),
        ]:
            draw.ellipse(box, fill=fill)
        draw.polygon(
            ((913, 505), (1066, 505), (1040, 616), (939, 616)),
            fill="#b66e48" if refined else "#bb9b57",
        )
        draw.line((913, 505, 1066, 505), fill="#87482e" if refined else "#937339", width=8)
        for x, label in [
            (78, "01 / Come as you are"),
            (474, "02 / Grow together"),
            (870, "03 / Take a moment"),
        ]:
            draw.line((x, 732, x + 328, 732), fill="#bdc5aa", width=1)
            self.text(draw, (x, 757), label, 19, "#38553b", 550)
        return image

    def poster_page(self, refined: bool) -> Image.Image:
        image = Image.new("RGB", (1320, 820), "#e8e6ee")
        draw = ImageDraw.Draw(image)
        color = "#473283" if refined else "#172f51"
        accent = "#d4b8ff" if refined else "#d1e9fd"
        rounded(draw, (352, 32, 970, 788), color, 0)
        self.text(draw, (393, 73), "AFTER HOURS", 25, accent, 750)
        self.text(draw, (393, 128), "Ways\nof seeing.", 84, "#f8f4e9", 650)
        draw.ellipse((425, 365, 873, 813), fill=accent)
        draw.ellipse((532, 379, 762, 765), fill=color)
        draw.rectangle((353, 722, 969, 788), fill=color)
        self.text(draw, (393, 734), "FRI 24 / 19:00", 22, accent, 700)
        self.text(draw, (932, 737), "EXHIBITION", 17, accent, 650, "ra")
        draw.rectangle((353, 789, 969, 819), fill="#e8e6ee")
        return image

    def frame(self, seconds: float) -> Image.Image:
        image = Image.new("RGB", SIZE, self.bg)
        draw = ImageDraw.Draw(image)
        draw.rectangle((0, 0, 1919, 75), fill="#fbfcf9")
        draw.line((0, 75, 1919, 75), fill="#d8ded5", width=1)
        self.text(draw, (34, 21), "Forma", 30, weight=800)
        self.text(draw, (200, 30), "/", 20, "#9aa49b")
        self.text(draw, (242, 28), "Untitled canvas", 21)
        rounded(draw, (1645, 20, 1770, 56), "#eef1e8", 18)
        self.text(draw, (1666, 27), "Preview", 17, weight=650)
        rounded(draw, (1789, 20, 1889, 56), "#243a2a", 18)
        self.text(draw, (1811, 27), "Share", 17, "#f7f9ed", 650)
        # Conversation column. Large authored UI text remains readable in camera closeups.
        draw.rectangle((0, 76, 500, 1079), fill="#fbfcf9")
        draw.line((500, 76, 500, 1080), fill="#d8ded5", width=1)
        self.text(draw, (35, 112), "CONVERSATION", 15, "#627164", 750)
        self.text(draw, (34, 174), "What are we making?", 28, weight=650)
        count = round(len(self.prompt) * max(0, min((seconds - 0.35) / 2.2, 1)))
        if seconds > 0.35:
            rounded(draw, (28, 237, 472, 382), "#edf1e5", 18)
            self.text(draw, (48, 268), self.prompt[:count], 29, weight=550)
            if seconds < 2.55:
                draw.rectangle(
                    (48 + min(count, 21) * 14, 336, 50 + min(count, 21) * 14, 363), fill="#718558"
                )
        if seconds > 2.85:
            self.text(draw, (37, 437), "F", 22, "#294629", 800)
            self.text(draw, (76, 436), "Forma", 22, weight=750)
            self.text(draw, (37, 488), "Finding a direction", 25, weight=650)
            for i, label in enumerate(
                ["Set the visual rhythm", "Build the layout", "Make it editable"]
            ):
                visible = seconds > 3.0 + i * 0.42
                if visible:
                    draw.ellipse(
                        (39, 545 + i * 44, 53, 559 + i * 44),
                        fill=self.accent if seconds > 4.3 else "#b5c4aa",
                    )
                    self.text(draw, (76, 538 + i * 44), label, 21, "#60715e")
        if seconds > 6.7:
            progress = min((seconds - 6.7) / 1.15, 1)
            rounded(draw, (28, 741, 472, 824), "#edf1e5", 18)
            self.text(
                draw,
                (48, 764),
                self.followup[: round(len(self.followup) * progress)],
                27,
                weight=550,
            )
        if seconds > 8.45:
            self.text(draw, (37, 867), "F", 22, "#294629", 800)
            self.text(draw, (76, 867), "Done. Still yours to edit.", 22, weight=650)
        rounded(draw, (28, 971, 472, 1044), "#ffffff", 18, "#d3dbcc")
        self.text(draw, (49, 995), "Ask Forma to design...", 23, "#8c9785")
        draw.ellipse((413, 988, 454, 1029), fill="#233d2b")
        draw.line((434, 1018, 434, 998), fill="#f9fbed", width=3)
        draw.line((425, 1007, 434, 998, 443, 1007), fill="#f9fbed", width=3)
        # Canvas chrome and a subtle dot grid.
        self.text(draw, (535, 104), "Canvas", 18, "#687561", 650)
        self.text(draw, (1864, 104), "100%", 17, "#687561", 500, "ra")
        for x in range(526, 1920, 28):
            for y in range(150, 1080, 28):
                draw.ellipse((x, y, x + 1, y + 1), fill="#d3dbcf")
        if seconds <= 4.15:
            self.text(draw, (1210, 509), "A little space for a big idea.", 27, "#7b8972", 500, "mm")
            rounded(draw, (1090, 556, 1330, 595), "#e7ecdf", 19)
            self.text(draw, (1210, 565), "Waiting for your direction", 16, "#738268", 600, "ma")
        else:
            alpha = ease((seconds - 4.15) / 0.65)
            page = self.initial_page
            if seconds >= 8.45:
                page = Image.blend(
                    self.initial_page, self.refined_page, smooth((seconds - 8.45) / 0.6)
                )
            if alpha < 1:
                page = Image.blend(Image.new("RGB", page.size, self.bg), page, alpha)
            image.paste(page, (551, 169 + round((1 - alpha) * 40)))
            draw = ImageDraw.Draw(image)
            draw.rectangle((551, 169, 1870, 988), outline="#a8ba9a", width=2)
            self.text(
                draw,
                (554, 140),
                "Landing / Desktop" if self.variant == "garden" else "Poster / Print",
                15,
                "#657a5d",
                600,
            )
            if seconds > 9.3:
                rounded(draw, (1570, 1008, 1868, 1047), "#fbfcf9", 18, "#d4ddcc")
                self.text(draw, (1590, 1018), "All changes on the canvas", 16, "#4a6344", 600)
        # A small, authored cursor follows the result rather than hiding key text.
        if 4.8 < seconds < 6.6 or seconds > 9.1:
            x = 1720 - int(100 * math.sin(seconds * 0.8))
            y = 926 - int(60 * math.cos(seconds * 0.8))
            draw.polygon(
                (
                    (x, y),
                    (x + 2, y + 31),
                    (x + 10, y + 22),
                    (x + 17, y + 37),
                    (x + 24, y + 33),
                    (x + 16, y + 19),
                    (x + 30, y + 18),
                ),
                fill="#14251b",
                outline="#ffffff",
                width=2,
            )
        return image


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("destination", nargs="?", default=".fixture")
    parser.add_argument("--variant", choices=["garden", "poster"], default="garden")
    args = parser.parse_args()
    folder = Path(args.destination)
    folder.mkdir(parents=True, exist_ok=True)
    destination = folder / f"{args.variant}.mp4"
    ffmpeg = shutil.which("ffmpeg")
    if not ffmpeg:
        raise SystemExit("FFmpeg is required on PATH")
    demo = Demo(args.variant)
    process = subprocess.Popen(
        [
            ffmpeg,
            "-v",
            "error",
            "-nostdin",
            "-y",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "-s",
            "1920x1080",
            "-r",
            str(FPS),
            "-i",
            "-",
            "-an",
            "-c:v",
            "libx264",
            "-crf",
            "16",
            "-preset",
            "fast",
            "-threads",
            "2",
            "-pix_fmt",
            "yuv420p",
            "-movflags",
            "+faststart",
            str(destination),
        ],
        stdin=subprocess.PIPE,
    )
    try:
        for index in range(FPS * SECONDS):
            process.stdin.write(demo.frame(index / FPS).tobytes())
        process.stdin.close()
        if process.wait(timeout=120):
            raise RuntimeError("FFmpeg could not encode the fixture")
    finally:
        if process.poll() is None:
            process.kill()
            process.wait()
    (folder / f"{args.variant}.json").write_text(
        json.dumps(
            {
                "footage": f"./{args.variant}.mp4",
                "brief": f"../inputs/{'forma' if args.variant == 'garden' else 'variation'}.json",
            },
            indent=2,
        )
        + "\n"
    )
    replay = {
        "footage": f"./{args.variant}.mp4",
        "brief": f"../inputs/{'forma' if args.variant == 'garden' else 'variation'}.json",
        "edit": f"../inputs/{'forma' if args.variant == 'garden' else 'variation'}-edit.json",
    }
    (folder / f"{args.variant}-replay.json").write_text(json.dumps(replay, indent=2) + "\n")
    demo.frame(10.8).save(folder / f"{args.variant}.png")
    print(f"Created {args.variant}.mp4: 12s, 1920x1080, 30fps, silent original fixture")


if __name__ == "__main__":
    main()
