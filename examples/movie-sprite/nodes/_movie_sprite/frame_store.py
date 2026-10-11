"""Constant-frame-memory access to a bounded, temporary raw video sequence."""

from __future__ import annotations

import math
from collections.abc import Iterator
from pathlib import Path

import numpy as np

from .geometry import Pixels


class FrameStore:
    """Read or write one frame per operation without mapping the entire sequence."""

    def __init__(
        self, path: Path, shape: tuple[int, int, int, int], *, create: bool = False
    ) -> None:
        self.path = path
        self.shape = shape
        self.frame_bytes = math.prod(shape[1:])
        expected_bytes = shape[0] * self.frame_bytes
        if create:
            with path.open("xb") as stream:
                stream.truncate(expected_bytes)
        elif path.stat().st_size != expected_bytes:
            raise ValueError("Raw frame store size differs from its declared dimensions")

    def __len__(self) -> int:
        return self.shape[0]

    def _offset(self, index: int) -> int:
        if index < 0:
            index += len(self)
        if not 0 <= index < len(self):
            raise IndexError("Frame index leaves the admitted sequence")
        return index * self.frame_bytes

    def __getitem__(self, index: int) -> Pixels:
        with self.path.open("rb") as stream:
            stream.seek(self._offset(index))
            data = stream.read(self.frame_bytes)
        if len(data) != self.frame_bytes:
            raise ValueError("Raw frame store contains an incomplete frame")
        return np.frombuffer(data, dtype=np.uint8).reshape(self.shape[1:])

    def __setitem__(self, index: int, frame: Pixels) -> None:
        if frame.shape != self.shape[1:] or frame.dtype != np.uint8:
            raise ValueError("Written frame differs from the frame store shape or pixel type")
        with self.path.open("r+b") as stream:
            stream.seek(self._offset(index))
            stream.write(frame.tobytes())

    def __iter__(self) -> Iterator[Pixels]:
        for index in range(len(self)):
            yield self[index]
