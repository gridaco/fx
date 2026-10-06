//! Budget reservation and settlement (spec/protocol.md §6.1 step 6 and 7; docs/wg/overview.md
//! "Retry and billing (ratified)"; spec/identity.md §12 money in micro-dollars).
//!
//! The run's ceiling is `--max-usd`, else the workflow's `budget.max_usd`, else the project's
//! (the plan's `ceiling`). A step's `budget:` is a scope `(owner path with repeat keys, max)`.
//! An instance carries the list of its scopes ([`Scopes`]); expansion records only the innermost
//! one today (`Instance::budget`), so the list has at most one entry until expansion records every
//! enclosing scope.
//!
//! - [`Ledger::reserve`]: every attempt of every paid call reserves the route's high price for
//!   its request, even when that is $0 (so every attempt is recorded). Refused when the amount
//!   exceeds what is left under the ceiling (`charged + held`) or under any scope (its spend plus
//!   its open holds): emits `budget_refused {node_id, needed_usd, remaining_usd, ceiling_usd}`
//!   (for a scope refusal, `ceiling_usd` is the scope's max) and returns a [`Refusal`] whose
//!   message is `run ceiling reached: <node_id> needs up to $<needed, 4 places> and $<remaining, 4
//!   places> is left`, or for a scope `step budget of <owner> reached: …`. Otherwise records the
//!   hold and emits `budget_reserved {node_id, amount_usd}`. The run ceiling is checked first,
//!   then each scope, innermost first; the first that refuses is reported.
//! - [`Ledger::settle`]: charges the reported cost, or the whole hold when none was reported, and
//!   emits `budget_settled {node_id, charged_usd, reported}`; the hold's scopes are charged the
//!   same amount. A hold that is no longer open (settled twice) charges only a reported cost
//!   above $0, and writes nothing otherwise.
//! - [`Ledger::replay`] (resuming): from earlier events, `budget_reserved` opens a hold,
//!   `budget_settled` closes it and adds its charge; every hold still open afterwards is charged
//!   in full (its process died and it may have billed). Scope spend is replayed from the same
//!   events, the scope of a hold found from its instance id (the `node_id` before its last `/`).
//!   An amount that is not a non-negative number reads as 0.
//! - Hold ids ([`hold_id`]): `<instance id>/<invocation id>.<n>`, unique in the folder.
//!
//! **What the log does not hold did not happen.** Events are written only when a log is attached,
//! and every one is written under the ledger's lock, so the log's order of reservations and
//! settlements is the ledger's. A reservation whose `budget_reserved` (or `budget_refused`) cannot
//! be written opens no hold: [`Ledger::reserve_recorded`] answers [`NotReserved::Unrecorded`],
//! and the call is refused before anything is sent. A settlement whose `budget_settled` cannot be
//! written still charges the run (the provider was paid), and [`Ledger::settle_recorded`] answers
//! [`Unrecorded`]. Either way the ledger keeps the first such failure ([`Ledger::fault`]): from
//! then on every reservation is refused as unrecorded, since the log can no longer be trusted to
//! hold what is spent; a resumed run then charges every hold whose settlement is missing in full.
//! The call path turns an unrecorded reservation or settlement into an engine fault that stops the
//! run. [`Ledger::reserve`] and [`Ledger::settle`] are the same without the distinction (an
//! unrecorded reservation is a [`Refusal`] whose message is the failure).
//!
//! Every amount is exact: [`Usd`] micro-dollars. A ceiling is never negative (refused while parsing
//! `--max-usd`).

use crate::events::{Event, EventLog};
use grida_fx_core::money::Usd;
use grida_fx_core::value::as_f64;
use serde_json::Value;
use std::sync::{Arc, Mutex};

/// The budget scopes an instance is inside, innermost first: `(owner path, max)`.
pub type Scopes = Vec<(String, Usd)>;

/// An open hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hold {
    pub node_id: String,
    pub amount: Usd,
    pub scopes: Vec<String>,
}

/// A refused reservation (`ceiling_exceeded`; its `data` carries `needed_usd` and
/// `remaining_usd`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub needed: Usd,
    pub remaining: Usd,
    pub message: String,
}

/// Why [`Ledger::reserve_recorded`] opened no hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotReserved {
    /// The ceiling or a step budget refused it (`ceiling_exceeded`).
    Refused(Refusal),
    /// The log could not record it, or an earlier event (module doc): an engine fault.
    Unrecorded(String),
}

/// A settlement the log could not record (module doc): the run was charged `charged` all the
/// same.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unrecorded {
    pub charged: Usd,
    pub reason: String,
}

#[derive(Debug, Default)]
struct State {
    charged: Usd,
    open: indexmap::IndexMap<String, Hold>,
    scope_spent: indexmap::IndexMap<String, Usd>,
    /// The first event the log could not write (module doc).
    fault: Option<String>,
}

impl State {
    /// Held by open reservations.
    fn held(&self) -> Usd {
        self.open.values().map(|hold| hold.amount).sum()
    }

    /// A scope's spend plus its open holds.
    fn scope_used(&self, owner: &str) -> Usd {
        let spent = self.scope_spent.get(owner).copied().unwrap_or(Usd::ZERO);
        let held: Usd = self
            .open
            .values()
            .filter(|hold| hold.scopes.iter().any(|scope| scope == owner))
            .map(|hold| hold.amount)
            .sum();
        spent + held
    }

    /// Adds a charge to the run and to each scope.
    fn charge(&mut self, amount: Usd, scopes: &[String]) {
        self.charged = self.charged + amount;
        for scope in scopes {
            let spent = self.scope_spent.entry(scope.clone()).or_insert(Usd::ZERO);
            *spent = *spent + amount;
        }
    }
}

/// One run invocation's ledger.
#[derive(Debug)]
pub struct Ledger {
    ceiling: Option<Usd>,
    events: Option<Arc<EventLog>>,
    state: Mutex<State>,
}

impl Ledger {
    pub fn new(ceiling: Option<Usd>, events: Option<Arc<EventLog>>) -> Ledger {
        Ledger {
            ceiling,
            events,
            state: Mutex::new(State::default()),
        }
    }

    /// Replays a folder's earlier events (module doc). `scopes_of` gives an instance id's scopes.
    pub fn replay(&self, prior: &[Value], scopes_of: &dyn Fn(&str) -> Scopes) {
        let mut open: indexmap::IndexMap<String, Usd> = indexmap::IndexMap::new();
        let mut settled: Vec<(String, Usd)> = Vec::new();
        for event in prior {
            let Some(node_id) = event.get("node_id").and_then(Value::as_str) else {
                continue;
            };
            match event.get("event").and_then(Value::as_str) {
                Some("budget_reserved") => {
                    open.insert(node_id.to_string(), amount(event.get("amount_usd")));
                }
                Some("budget_settled") => {
                    open.shift_remove(node_id);
                    settled.push((node_id.to_string(), amount(event.get("charged_usd"))));
                }
                _ => {}
            }
        }
        // A hold an earlier invocation never settled may have billed all of it: charged, not
        // released.
        settled.extend(open);
        let mut state = self.lock();
        for (node_id, charged) in settled {
            let scopes: Vec<String> = scopes_of(instance_of(&node_id))
                .into_iter()
                .map(|(owner, _)| owner)
                .collect();
            state.charge(charged, &scopes);
        }
    }

    /// Reserves `amount` for `node_id` under the ceiling and `scopes` (module doc). An
    /// unrecorded reservation is refused too, with the failure as the refusal's message.
    pub fn reserve(&self, node_id: String, amount: Usd, scopes: &Scopes) -> Result<Hold, Refusal> {
        let needed = non_negative(amount);
        self.reserve_recorded(node_id, amount, scopes)
            .map_err(|not| match not {
                NotReserved::Refused(refusal) => refusal,
                NotReserved::Unrecorded(message) => Refusal {
                    needed,
                    remaining: Usd::ZERO,
                    message,
                },
            })
    }

    /// Reserves `amount` for `node_id` under the ceiling and `scopes` (module doc); a hold is
    /// opened only once its `budget_reserved` is written.
    pub fn reserve_recorded(
        &self,
        node_id: String,
        amount: Usd,
        scopes: &Scopes,
    ) -> Result<Hold, NotReserved> {
        let amount = non_negative(amount);
        let mut state = self.lock();
        if let Some(fault) = &state.fault {
            return Err(NotReserved::Unrecorded(fault.clone()));
        }
        if state.open.contains_key(&node_id) {
            // Hold ids are unique by construction; a second hold under one id would lose the
            // first one's amount, so it is refused before anything is sent.
            return Err(NotReserved::Refused(Refusal {
                needed: amount,
                remaining: Usd::ZERO,
                message: format!("{node_id} already holds a reservation"),
            }));
        }
        let mut refusal = None;
        if let Some(ceiling) = self.ceiling {
            let remaining = minus(ceiling, state.charged + state.held());
            if amount > remaining {
                refusal = Some((
                    remaining,
                    ceiling,
                    format!(
                        "run ceiling reached: {node_id} needs up to ${} and ${} is left",
                        dollars_4(amount),
                        dollars_4(remaining)
                    ),
                ));
            }
        }
        if refusal.is_none() {
            for (owner, max) in scopes {
                let max = non_negative(*max);
                let remaining = minus(max, state.scope_used(owner));
                if amount > remaining {
                    refusal = Some((
                        remaining,
                        max,
                        format!(
                            "step budget of {owner} reached: {node_id} needs up to ${} and ${} \
                             is left",
                            dollars_4(amount),
                            dollars_4(remaining)
                        ),
                    ));
                    break;
                }
            }
        }
        if let Some((remaining, ceiling, message)) = refusal {
            let event = Event::BudgetRefused {
                node_id,
                needed_usd: amount,
                remaining_usd: remaining,
                ceiling_usd: Some(ceiling),
            };
            if let Err(fault) = self.emit(&mut state, &event) {
                return Err(NotReserved::Unrecorded(fault));
            }
            return Err(NotReserved::Refused(Refusal {
                needed: amount,
                remaining,
                message,
            }));
        }
        let event = Event::BudgetReserved {
            node_id: node_id.clone(),
            amount_usd: amount,
        };
        if let Err(fault) = self.emit(&mut state, &event) {
            // Nothing may be sent for a hold the log does not hold.
            return Err(NotReserved::Unrecorded(fault));
        }
        let hold = Hold {
            node_id: node_id.clone(),
            amount,
            scopes: scopes.iter().map(|(owner, _)| owner.clone()).collect(),
        };
        state.open.insert(node_id, hold.clone());
        Ok(hold)
    }

    /// Settles a hold at the reported cost, or in full when `None` (module doc). Returns what was
    /// charged, recorded or not.
    pub fn settle(&self, hold: Hold, reported: Option<Usd>) -> Usd {
        match self.settle_recorded(hold, reported) {
            Ok(charged) => charged,
            Err(unrecorded) => unrecorded.charged,
        }
    }

    /// Settles a hold at the reported cost, or in full when `None` (module doc): what was
    /// charged, or [`Unrecorded`] when its `budget_settled` could not be written.
    pub fn settle_recorded(&self, hold: Hold, reported: Option<Usd>) -> Result<Usd, Unrecorded> {
        let reported = reported.map(non_negative);
        let mut state = self.lock();
        let open = state.open.shift_remove(&hold.node_id);
        let charged = match (&open, reported) {
            (Some(_), Some(cost)) => cost,
            (Some(open), None) => open.amount,
            (None, Some(cost)) if cost > Usd::ZERO => cost,
            (None, _) => return Ok(Usd::ZERO),
        };
        let scopes = open.map_or(hold.scopes, |open| open.scopes);
        state.charge(charged, &scopes);
        let event = Event::BudgetSettled {
            node_id: hold.node_id,
            charged_usd: charged,
            reported: reported.is_some(),
        };
        match self.emit(&mut state, &event) {
            Ok(()) => Ok(charged),
            Err(reason) => Err(Unrecorded { charged, reason }),
        }
    }

    /// The first event this ledger could not write, if any (module doc).
    pub fn fault(&self) -> Option<String> {
        self.lock().fault.clone()
    }

    pub fn ceiling(&self) -> Option<Usd> {
        self.ceiling
    }

    /// Charged so far in this folder, every invocation included.
    pub fn charged(&self) -> Usd {
        self.lock().charged
    }

    /// Held by open reservations.
    pub fn held(&self) -> Usd {
        self.lock().held()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Writes an event when a log is attached, with the state locked so the log's order of
    /// reservations and settlements is the ledger's. A failure is kept as the ledger's fault
    /// (the first one wins) and returned as a sentence.
    fn emit(&self, state: &mut State, event: &Event) -> Result<(), String> {
        let Some(log) = &self.events else {
            return Ok(());
        };
        log.emit(event).map_err(|error| {
            let reason = format!(
                "the run's events.jsonl could not record {} of {}: {error}",
                event.name(),
                held_by(event)
            );
            state.fault.get_or_insert_with(|| reason.clone());
            reason
        })
    }
}

/// The hold a budget event names.
fn held_by(event: &Event) -> &str {
    match event {
        Event::BudgetReserved { node_id, .. }
        | Event::BudgetSettled { node_id, .. }
        | Event::BudgetRefused { node_id, .. } => node_id,
        _ => "a hold",
    }
}

/// `<instance id>/<invocation id>.<n>` (module doc).
pub fn hold_id(instance_id: &str, invocation_id: &str, n: u64) -> String {
    format!("{instance_id}/{invocation_id}.{n}")
}

/// The instance a hold id belongs to: everything before its last `/` (the whole id when it has
/// none).
fn instance_of(node_id: &str) -> &str {
    node_id
        .rsplit_once('/')
        .map_or(node_id, |(instance, _)| instance)
}

/// An amount read back from an event: a non-negative number of dollars, else 0. Exact up to the
/// micro-dollar (identity.md §12); anything finer rounds half to even.
fn amount(value: Option<&Value>) -> Usd {
    let Some(Value::Number(n)) = value else {
        return Usd::ZERO;
    };
    if let Ok(usd) = Usd::from_value(&Value::Number(n.clone())) {
        return usd;
    }
    let dollars = as_f64(n);
    if !dollars.is_finite() || dollars <= 0.0 {
        return Usd::ZERO;
    }
    let micros = (dollars * 1_000_000.0).round_ties_even();
    if micros >= i64::MAX as f64 {
        Usd(i64::MAX)
    } else {
        Usd(micros as i64)
    }
}

fn non_negative(amount: Usd) -> Usd {
    Usd(amount.0.max(0))
}

/// `a - b`, never below 0.
fn minus(a: Usd, b: Usd) -> Usd {
    Usd(a.0.saturating_sub(b.0).max(0))
}

/// Dollars with 4 decimal places from exact micro-dollars, rounded half to even
/// (identity.md §12): 40000 → `0.0400`, 123450 → `0.1234`, 123451 → `0.1235`.
pub(crate) fn dollars_4(amount: Usd) -> String {
    let micros = i128::from(amount.0);
    let negative = micros < 0;
    let micros = micros.unsigned_abs();
    let (quotient, remainder) = (micros / 100, micros % 100);
    let rounded = match remainder.cmp(&50) {
        std::cmp::Ordering::Less => quotient,
        std::cmp::Ordering::Greater => quotient + 1,
        std::cmp::Ordering::Equal if quotient % 2 == 0 => quotient,
        std::cmp::Ordering::Equal => quotient + 1,
    };
    let sign = if negative && rounded != 0 { "-" } else { "" };
    format!("{sign}{}.{:04}", rounded / 10_000, rounded % 10_000)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn four_places_from_exact_micros() {
        assert_eq!(dollars_4(Usd(0)), "0.0000");
        assert_eq!(dollars_4(Usd(40_000)), "0.0400");
        assert_eq!(dollars_4(Usd(1_234_567)), "1.2346");
        assert_eq!(dollars_4(Usd(123_450)), "0.1234");
        assert_eq!(dollars_4(Usd(123_550)), "0.1236");
        assert_eq!(dollars_4(Usd(123_451)), "0.1235");
        assert_eq!(dollars_4(Usd(12_000_000)), "12.0000");
        assert_eq!(dollars_4(Usd(99)), "0.0001");
        assert_eq!(dollars_4(Usd(49)), "0.0000");
    }

    #[test]
    fn amounts_read_back_from_events() {
        assert_eq!(amount(Some(&json!(0.04))), Usd(40_000));
        assert_eq!(amount(Some(&json!(2))), Usd(2_000_000));
        assert_eq!(amount(Some(&json!(-1))), Usd::ZERO);
        assert_eq!(amount(Some(&json!("0.04"))), Usd::ZERO);
        assert_eq!(amount(Some(&json!(true))), Usd::ZERO);
        assert_eq!(amount(Some(&Value::Null)), Usd::ZERO);
        assert_eq!(amount(None), Usd::ZERO);
        // Finer than a micro-dollar (never written by FX) rounds to the nearest one.
        assert_eq!(amount(Some(&json!(0.0000014))), Usd(1));
    }

    #[test]
    fn instance_of_a_hold() {
        assert_eq!(instance_of("draw#1/0123456789abcdef.3"), "draw#1");
        assert_eq!(
            instance_of("g['a/b'].draw#1/0123456789abcdef.1"),
            "g['a/b'].draw#1"
        );
        assert_eq!(instance_of("plain"), "plain");
    }
}
