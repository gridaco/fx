//! The order key (spec/layout.md §4 rule 4). Arrival and scheduling order are never read.

use super::input::{Input, Instance, Scope};

/// A slot's place in its container: its declared `order`; in a record without one, the first
/// position of its instances in the plan's `instances`, then its pending entry's position, then
/// its address in code-point order.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct SlotKey {
    rank: u8,
    index: u64,
    pub address: String,
}

impl SlotKey {
    pub(crate) fn new(
        address: &str,
        declared: Option<u64>,
        planned: Option<usize>,
        pending: Option<usize>,
    ) -> Self {
        let (rank, index) = match (declared, planned, pending) {
            (Some(order), _, _) => (0, order),
            (None, Some(index), _) => (1, index as u64),
            (None, None, Some(index)) => (2, index as u64),
            (None, None, None) => (3, 0),
        };
        Self {
            rank,
            index,
            address: address.to_string(),
        }
    }
}

/// A member's place in its slot: its position in the newest recorded expansion order; missing
/// from it, its key in code-point order, then its take number by number, then its id (D2). A
/// frame or workflow card takes the smallest key among its descendants. A card without member
/// descendants falls back to its absent descendants' plan positions, then its pending entries'
/// positions, then its own path, take and id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct MemberKey<'a> {
    tier: u8,
    index: usize,
    key: Option<&'a str>,
    take: &'a [u64],
    id: &'a str,
}

impl<'a> MemberKey<'a> {
    pub(crate) fn instance(input: &Input, instance: &'a Instance) -> Self {
        match input.expansion.get(&instance.id) {
            Some(&index) => Self::at(0, index),
            None => Self {
                tier: 1,
                index: 0,
                key: instance.key.as_deref(),
                take: &instance.take,
                id: &instance.id,
            },
        }
    }

    /// An absent descendant at its plan position.
    pub(crate) fn absent(index: usize) -> Self {
        Self::at(2, index)
    }

    /// A pending descendant at its position.
    pub(crate) fn pending(index: usize) -> Self {
        Self::at(3, index)
    }

    /// A card with nothing below it that a record orders.
    pub(crate) fn scope(scope: &'a Scope) -> Self {
        Self {
            tier: 4,
            index: 0,
            key: Some(&scope.path),
            take: &scope.take,
            id: &scope.id,
        }
    }

    fn at(tier: u8, index: usize) -> Self {
        Self {
            tier,
            index,
            key: None,
            take: &[],
            id: "",
        }
    }
}
