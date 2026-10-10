//! The automatic cell rule (spec/layout.md §4, rules 5–8 and 10). Rule 9, authored cells,
//! belongs to the layout file (Ship B): it would place authored slots between rule 8 and rule
//! 10, on the coordinates [`cells`] computes before collapsing, which is why the collapse is a
//! separate step here.
//!
//! The generic graph algorithms come from `petgraph`: union-find for clusters, Tarjan's strongly
//! connected components for cycles, and a topological order for depths.

use petgraph::algo::{tarjan_scc, toposort};
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::unionfind::UnionFind;
use std::collections::{BTreeMap, BTreeSet};

/// Rule 8: a depth with more slots than this splits into sub-columns. The engine's constant;
/// no file key changes it.
const WRAP: usize = 3;

/// The `(column, row)` of one container's slots. `slots` are indexed in order key; `edges` are
/// the distinct placement edges between two of them (rule 3), source first. Reads no size and
/// no member.
pub(crate) fn cells(slots: usize, edges: &[(usize, usize)]) -> Vec<(u32, u32)> {
    collapse(automatic(slots, edges))
}

/// Rules 5–8: every slot's column and row before empty tracks collapse.
fn automatic(slots: usize, edges: &[(usize, usize)]) -> Vec<(u32, u32)> {
    let kept = acyclic(slots, edges);
    let depth = depths(slots, &kept);
    let mut predecessors = vec![Vec::new(); slots];
    for &(source, target) in &kept {
        predecessors[target].push(source);
    }
    let mut placed: Vec<(u32, u32)> = vec![(0, 0); slots];
    let mut column = 0_u32;
    for cluster in clusters(slots, edges) {
        let deepest = cluster.iter().map(|&slot| depth[slot]).max().unwrap_or(0);
        let mut levels = vec![Vec::new(); deepest + 1];
        for slot in cluster {
            levels[depth[slot]].push(slot);
        }
        // Rule 7: depths in increasing order, so every predecessor's row is final (rule 8).
        for level in levels {
            let mut ranked: Vec<(u32, usize)> = level
                .into_iter()
                .map(|slot| {
                    let preferred = predecessors[slot]
                        .iter()
                        .map(|&predecessor| placed[predecessor].1)
                        .min()
                        .unwrap_or(0);
                    (preferred, slot)
                })
                .collect();
            ranked.sort_unstable();
            if ranked.len() <= WRAP {
                let mut taken = BTreeSet::new();
                for (preferred, slot) in ranked {
                    let mut row = preferred;
                    while !taken.insert(row) {
                        row += 1;
                    }
                    placed[slot] = (column, row);
                }
                column += 1;
            } else {
                // Rule 8: sub-columns of up to three, top to bottom in rule 7's order, from the
                // depth's smallest preferred row (D3).
                let first = ranked[0].0;
                for (position, &(_, slot)) in ranked.iter().enumerate() {
                    placed[slot] = (
                        column + (position / WRAP) as u32,
                        first + (position % WRAP) as u32,
                    );
                }
                column += ranked.len().div_ceil(WRAP) as u32;
            }
        }
    }
    placed
}

/// Rule 5: connected components, edge direction ignored; the largest first, ties by order key.
/// Each lists its slots in order key.
fn clusters(slots: usize, edges: &[(usize, usize)]) -> Vec<Vec<usize>> {
    let mut sets = UnionFind::new(slots);
    for &(source, target) in edges {
        sets.union(source, target);
    }
    let mut grouped: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for slot in 0..slots {
        grouped.entry(sets.find(slot)).or_default().push(slot);
    }
    let mut clusters: Vec<Vec<usize>> = grouped.into_values().collect();
    clusters.sort_by(|left, right| right.len().cmp(&left.len()).then(left[0].cmp(&right[0])));
    clusters
}

/// Rule 6, cycles: inside one strongly connected component, an edge to a slot earlier in the
/// order key is ignored for depth (it is still drawn). Every kept edge inside a component goes
/// forward in the order key, so what remains is acyclic.
fn acyclic(slots: usize, edges: &[(usize, usize)]) -> Vec<(usize, usize)> {
    let graph = graph(slots, edges);
    let mut component = vec![0; slots];
    for (index, members) in tarjan_scc(&graph).into_iter().enumerate() {
        for node in members {
            component[node.index()] = index;
        }
    }
    edges
        .iter()
        .copied()
        .filter(|&(source, target)| component[source] != component[target] || source < target)
        .collect()
}

/// Rule 6: as soon as possible, 0 with no predecessor, else 1 + the deepest predecessor.
fn depths(slots: usize, edges: &[(usize, usize)]) -> Vec<usize> {
    let graph = graph(slots, edges);
    let order = toposort(&graph, None).expect("edges to earlier slots in a cycle are removed");
    let mut depth = vec![0; slots];
    for node in order {
        for next in graph.neighbors(node) {
            depth[next.index()] = depth[next.index()].max(depth[node.index()] + 1);
        }
    }
    depth
}

fn graph(slots: usize, edges: &[(usize, usize)]) -> DiGraph<(), ()> {
    let mut graph = DiGraph::with_capacity(slots, edges.len());
    for _ in 0..slots {
        graph.add_node(());
    }
    for &(source, target) in edges {
        graph.add_edge(NodeIndex::new(source), NodeIndex::new(target), ());
    }
    graph
}

/// Rule 10: a column or row that holds no slot is removed; order is kept.
fn collapse(placed: Vec<(u32, u32)>) -> Vec<(u32, u32)> {
    let rank =
        |values: BTreeSet<u32>| -> BTreeMap<u32, u32> { values.into_iter().zip(0..).collect() };
    let columns = rank(placed.iter().map(|cell| cell.0).collect());
    let rows = rank(placed.iter().map(|cell| cell.1).collect());
    placed
        .into_iter()
        .map(|(column, row)| (columns[&column], rows[&row]))
        .collect()
}
