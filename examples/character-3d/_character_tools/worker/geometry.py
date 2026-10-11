"""Measured mesh operations in canonical glTF coordinates; no anatomy heuristics."""

from __future__ import annotations

import itertools
from collections import defaultdict
from typing import Any

import numpy as np
from numpy.typing import ArrayLike, NDArray


def bounds(points: NDArray[Any]) -> dict[str, list[float]]:
    if not len(points) or not np.isfinite(points).all():
        raise ValueError("Expected nonempty finite geometry")
    lo, hi = (points.min(0), points.max(0))
    return {"min": lo.tolist(), "max": hi.tolist(), "dimensions": (hi - lo).tolist()}


class UnionFind:
    def __init__(self, size: int) -> None:
        self.parents = np.arange(size)

    def find(self, value: int) -> int:
        while self.parents[value] != value:
            self.parents[value] = self.parents[self.parents[value]]
            value = self.parents[value]
        return int(value)

    def join(self, left: int, right: int) -> None:
        a, b = (self.find(left), self.find(right))
        if a != b:
            self.parents[max(a, b)] = min(a, b)


def topology(
    points: NDArray[Any], triangles: NDArray[Any], tolerance: float
) -> tuple[dict[str, Any], NDArray[Any]]:
    """Euclidean proximity weld for analysis only; never modify input arrays.

    Proximity clusters are transitive. A chain can have diameter larger than
    tolerance; this is stated in the report rather than treated as exact welding.
    """
    referenced = np.unique(triangles)
    weld = UnionFind(len(points))
    cells: dict[tuple[Any, ...], list[int]] = defaultdict(list)
    for index in referenced:
        point = points[index]
        key = tuple(np.floor(point / tolerance).astype(np.int64)) if tolerance else tuple(point)
        neighbors = (
            (
                tuple(key[a] + offset[a] for a in range(3))
                for offset in itertools.product((-1, 0, 1), repeat=3)
            )
            if tolerance
            else [key]
        )
        for cell in neighbors:
            for other in cells.get(cell, []):
                if np.linalg.norm(point - points[other]) <= tolerance:
                    weld.join(index, other)
        cells[key].append(int(index))
    canonical = np.arange(len(points))
    for index in referenced:
        canonical[index] = weld.find(index)
    faces = canonical[triangles]
    graph = UnionFind(len(points))
    for a, b, c in faces:
        graph.join(a, b)
        graph.join(a, c)
    groups: dict[int, list[int]] = defaultdict(list)
    for index in referenced:
        groups[graph.find(canonical[index])].append(int(index))
    components = []
    membership = np.full(len(points), -1, dtype=np.int32)
    for component_index, indices in enumerate(
        sorted(groups.values(), key=lambda value: (-len(value), value[0]))
    ):
        membership[indices] = component_index
        components.append(
            {
                "index": component_index,
                "vertex_count": len(indices),
                "bounds": bounds(points[indices]),
            }
        )
    valid = np.array([len(set(face)) == 3 for face in faces], dtype=bool)
    edge_counts: dict[tuple[int, ...], int] = defaultdict(int)
    for face in faces[valid]:
        for a, b in ((face[0], face[1]), (face[1], face[2]), (face[2], face[0])):
            edge_counts[tuple(sorted((int(a), int(b))))] += 1
    return (
        {
            "tolerance": tolerance,
            "component_count": len(components),
            "referenced_vertices": len(referenced),
            "unreferenced_vertices": len(points) - len(referenced),
            "welded_vertices": len(set(canonical[referenced])),
            "collapsed_triangles_after_analysis_weld": int((~valid).sum()),
            "boundary_edges": sum(count == 1 for count in edge_counts.values()),
            "nonmanifold_edges": sum(count > 2 for count in edge_counts.values()),
            "components": components,
        },
        membership,
    )


def indexed_topology(points: NDArray[Any], triangles: NDArray[Any]) -> dict[str, int]:
    """Index connectivity does not merge coincident records."""
    graph = UnionFind(len(points))
    edges: dict[tuple[int, ...], int] = defaultdict(int)
    for a, b, c in triangles:
        graph.join(a, b)
        graph.join(a, c)
        for x, y in ((a, b), (b, c), (c, a)):
            edges[tuple(sorted((int(x), int(y))))] += 1
    referenced = np.unique(triangles)
    return {
        "component_count": len({graph.find(i) for i in referenced}),
        "boundary_edges": sum(count == 1 for count in edges.values()),
        "nonmanifold_edges": sum(count > 2 for count in edges.values()),
    }


def slice_mesh(
    points: NDArray[Any], triangles: NDArray[Any], point: ArrayLike, normal: ArrayLike
) -> dict[str, Any]:
    point, normal = (np.asarray(point), np.asarray(normal, dtype=float))
    normal /= np.linalg.norm(normal)
    diagonal = np.linalg.norm(np.ptp(points, axis=0))
    epsilon = max(diagonal * 1e-09, 1e-12)
    distances = (points - point) @ normal
    segments, indices, coplanar = ([], [], [])
    for index, face in enumerate(triangles):
        signed = distances[face]
        if np.all(np.abs(signed) <= epsilon):
            coplanar.append(index)
            continue
        if np.all(signed > epsilon) or np.all(signed < -epsilon):
            continue
        hits = []
        for a, b in ((0, 1), (1, 2), (2, 0)):
            pa, pb = (points[face[a]], points[face[b]])
            da, db = (signed[a], signed[b])
            if abs(da) <= epsilon:
                hits.append(pa)
            if da * db < 0:
                hits.append(pa + (pb - pa) * da / (da - db))
        distinct: list[NDArray[Any]] = []
        for hit in hits:
            if not any(np.linalg.norm(hit - other) <= epsilon for other in distinct):
                distinct.append(hit)
        if len(distinct) == 2:
            segments.append(distinct)
            indices.append(index)
    return {
        "point": point.tolist(),
        "normal": normal.tolist(),
        "epsilon": epsilon,
        "segment_count": len(segments),
        "segments": np.asarray(segments).reshape(-1, 2, 3).tolist(),
        "triangle_indices": indices,
        "coplanar_triangles": coplanar,
        "limitations": (
            "Unordered triangle-plane segments, not reconstructed contour l"
            "oops; tangential point contacts omitted."
        ),
    }
