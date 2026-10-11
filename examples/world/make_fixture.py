"""Draw a synthetic reference for offline planning; never replace the showcase input."""

import json
from pathlib import Path

from PIL import Image, ImageDraw


def make_fixture() -> Path:
    home = Path(__file__).resolve().parent
    target = home / ".fixture" / "meadow.png"
    target.parent.mkdir(exist_ok=True)
    image = Image.new("RGB", (480, 320), "#789bbb")
    draw = ImageDraw.Draw(image)
    draw.rectangle((0, 210, 479, 319), fill="#d4aa55")
    draw.ellipse((315, 90, 435, 230), fill="#315b4a")
    draw.rectangle((368, 180, 382, 260), fill="#675440")
    image.save(target, format="PNG")
    inputs = {
        "universe_id": "windward_meadow",
        "title": "Where the Meadow Listens",
        "medium": "anime_2d",
        "poster": "meadow.png",
        "synopsis": "../inputs/windward_meadow/synopsis.md",
        "direction": "../inputs/windward_meadow/expansion-direction.md",
        "census": {"min_entities": 6, "max_entities": 6},
        "rights": {"status": "unreviewed", "basis": ["Synthesized offline test fixture."]},
    }
    (target.parent / "inputs.json").write_text(json.dumps(inputs, indent=2) + "\n")
    return target


if __name__ == "__main__":
    print(make_fixture().relative_to(Path(__file__).resolve().parent))
