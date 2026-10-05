//! Budgets (spec/protocol.md §6.1; docs/wg/overview.md "Retry and billing (ratified)"): holds,
//! settlements, refusals under the run ceiling and step budgets, and replay on resume.

use grida_fx_core::money::Usd;
use grida_fx_runtime::events::{EventLog, read_events};
use grida_fx_runtime::ledger::{Hold, Ledger, Refusal, Scopes, hold_id};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;

const PLAN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn usd(dollars: &str) -> Usd {
    Usd::parse(dollars).unwrap()
}

/// A ledger writing to a log in a temporary folder.
struct Logged {
    _dir: tempfile::TempDir,
    path: PathBuf,
    ledger: Ledger,
}

impl Logged {
    fn new(ceiling: Option<&str>) -> Logged {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        let log = Arc::new(EventLog::open(&path, "0123456789abcdef", PLAN).unwrap());
        Logged {
            _dir: dir,
            path,
            ledger: Ledger::new(ceiling.map(usd), Some(log)),
        }
    }

    /// The events written so far, without the envelope.
    fn events(&self) -> Vec<Value> {
        read_events(&self.path)
            .unwrap()
            .into_iter()
            .map(|mut event| {
                let map = event.as_object_mut().unwrap();
                for envelope in ["kind", "invocation_id", "plan", "offset_ms"] {
                    map.remove(envelope);
                }
                event
            })
            .collect()
    }
}

fn none() -> Scopes {
    Vec::new()
}

#[test]
fn hold_ids_name_the_instance_and_the_invocation() {
    assert_eq!(
        hold_id("draw#1", "0123456789abcdef", 3),
        "draw#1/0123456789abcdef.3"
    );
}

#[test]
fn a_reservation_is_settled_at_the_reported_cost() {
    let logged = Logged::new(Some("0.10"));
    let ledger = &logged.ledger;
    let hold = ledger
        .reserve("a#1/i.1".into(), usd("0.04"), &none())
        .unwrap();
    assert_eq!(
        hold,
        Hold {
            node_id: "a#1/i.1".into(),
            amount: usd("0.04"),
            scopes: vec![],
        }
    );
    assert_eq!(ledger.held(), usd("0.04"));
    assert_eq!(ledger.charged(), Usd::ZERO);
    assert_eq!(ledger.settle(hold, Some(usd("0.013"))), usd("0.013"));
    assert_eq!(ledger.held(), Usd::ZERO);
    assert_eq!(ledger.charged(), usd("0.013"));
    assert_eq!(ledger.ceiling(), Some(usd("0.10")));
    assert_eq!(
        logged.events(),
        [
            json!({"event": "budget_reserved", "node_id": "a#1/i.1", "amount_usd": 0.04}),
            json!({"event": "budget_settled", "node_id": "a#1/i.1", "charged_usd": 0.013, "reported": true}),
        ]
    );
}

#[test]
fn an_unreported_cost_charges_the_whole_hold() {
    let logged = Logged::new(None);
    let ledger = &logged.ledger;
    let hold = ledger
        .reserve("a#1/i.1".into(), usd("0.04"), &none())
        .unwrap();
    assert_eq!(ledger.settle(hold, None), usd("0.04"));
    assert_eq!(ledger.charged(), usd("0.04"));
    assert_eq!(
        logged.events()[1],
        json!({"event": "budget_settled", "node_id": "a#1/i.1", "charged_usd": 0.04, "reported": false})
    );
}

#[test]
fn a_free_attempt_is_recorded_too() {
    let logged = Logged::new(Some("0"));
    let ledger = &logged.ledger;
    let hold = ledger
        .reserve("a#1/i.1".into(), Usd::ZERO, &none())
        .unwrap();
    assert_eq!(ledger.settle(hold, Some(Usd::ZERO)), Usd::ZERO);
    assert_eq!(
        logged.events(),
        [
            json!({"event": "budget_reserved", "node_id": "a#1/i.1", "amount_usd": 0}),
            json!({"event": "budget_settled", "node_id": "a#1/i.1", "charged_usd": 0, "reported": true}),
        ]
    );
    // A cost reported on a free route is still charged, and then nothing is left.
    let hold = ledger
        .reserve("a#1/i.2".into(), Usd::ZERO, &none())
        .unwrap();
    assert_eq!(ledger.settle(hold, Some(usd("0.002"))), usd("0.002"));
    let refused = ledger
        .reserve("a#1/i.3".into(), usd("0.001"), &none())
        .unwrap_err();
    assert_eq!(refused.remaining, Usd::ZERO);
}

#[test]
fn the_run_ceiling_refuses_what_does_not_fit() {
    let logged = Logged::new(Some("0.05"));
    let ledger = &logged.ledger;
    let first = ledger
        .reserve("a#1/i.1".into(), usd("0.04"), &none())
        .unwrap();
    let refused = ledger
        .reserve("b#1/i.2".into(), usd("0.04"), &none())
        .unwrap_err();
    assert_eq!(
        refused,
        Refusal {
            needed: usd("0.04"),
            remaining: usd("0.01"),
            message: "run ceiling reached: b#1/i.2 needs up to $0.0400 and $0.0100 is left".into(),
        }
    );
    // What fits exactly is admitted.
    let second = ledger
        .reserve("c#1/i.3".into(), usd("0.01"), &none())
        .unwrap();
    assert_eq!(ledger.held(), usd("0.05"));
    // Settling below the hold frees the rest.
    ledger.settle(first, Some(usd("0.01")));
    ledger.settle(second, None);
    assert_eq!(ledger.charged(), usd("0.02"));
    let third = ledger
        .reserve("b#1/i.4".into(), usd("0.03"), &none())
        .unwrap();
    ledger.settle(third, None);
    assert_eq!(ledger.charged(), usd("0.05"));
    let events = logged.events();
    assert_eq!(
        events[1],
        json!({"event": "budget_refused", "node_id": "b#1/i.2", "needed_usd": 0.04,
               "remaining_usd": 0.01, "ceiling_usd": 0.05})
    );
    let names: Vec<&str> = events
        .iter()
        .map(|e| e["event"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "budget_reserved",
            "budget_refused",
            "budget_reserved",
            "budget_settled",
            "budget_settled",
            "budget_reserved",
            "budget_settled",
        ]
    );
}

#[test]
fn money_in_messages_has_four_places() {
    let logged = Logged::new(Some("1.23456"));
    let refused = logged
        .ledger
        .reserve("a#1/i.1".into(), usd("2.000049"), &none())
        .unwrap_err();
    assert_eq!(
        refused.message,
        "run ceiling reached: a#1/i.1 needs up to $2.0000 and $1.2346 is left"
    );
}

#[test]
fn a_step_budget_refuses_with_its_own_max() {
    let logged = Logged::new(Some("1"));
    let ledger = &logged.ledger;
    let scene_a: Scopes = vec![("scene['a']".into(), usd("0.05"))];
    let scene_b: Scopes = vec![("scene['b']".into(), usd("0.05"))];
    let first = ledger
        .reserve("scene['a'].draw#1/i.1".into(), usd("0.03"), &scene_a)
        .unwrap();
    assert_eq!(first.scopes, ["scene['a']"]);
    let refused = ledger
        .reserve("scene['a'].fix#1/i.2".into(), usd("0.03"), &scene_a)
        .unwrap_err();
    assert_eq!(
        refused.message,
        "step budget of scene['a'] reached: scene['a'].fix#1/i.2 needs up to $0.0300 and $0.0200 is left"
    );
    assert_eq!(refused.needed, usd("0.03"));
    assert_eq!(refused.remaining, usd("0.02"));
    // Another scope has its own budget.
    let other = ledger
        .reserve("scene['b'].draw#1/i.3".into(), usd("0.03"), &scene_b)
        .unwrap();
    // Spend counts once a hold is settled.
    ledger.settle(first, Some(usd("0.01")));
    let next = ledger
        .reserve("scene['a'].fix#1/i.4".into(), usd("0.04"), &scene_a)
        .unwrap();
    let refused = ledger
        .reserve("scene['a'].fix#1/i.5".into(), usd("0.000001"), &scene_a)
        .unwrap_err();
    assert_eq!(refused.remaining, Usd::ZERO);
    ledger.settle(next, None);
    ledger.settle(other, None);
    assert_eq!(ledger.charged(), usd("0.08"));
    let refusals: Vec<Value> = logged
        .events()
        .into_iter()
        .filter(|e| e["event"] == "budget_refused")
        .collect();
    assert_eq!(
        refusals,
        [
            json!({"event": "budget_refused", "node_id": "scene['a'].fix#1/i.2", "needed_usd": 0.03,
                   "remaining_usd": 0.02, "ceiling_usd": 0.05}),
            json!({"event": "budget_refused", "node_id": "scene['a'].fix#1/i.5", "needed_usd": 0.000001,
                   "remaining_usd": 0, "ceiling_usd": 0.05}),
        ]
    );
}

#[test]
fn the_run_ceiling_is_checked_before_a_step_budget() {
    let logged = Logged::new(Some("0.01"));
    let scope: Scopes = vec![("scene".into(), usd("0.02"))];
    let refused = logged
        .ledger
        .reserve("scene.draw#1/i.1".into(), usd("0.03"), &scope)
        .unwrap_err();
    assert!(
        refused.message.starts_with("run ceiling reached: "),
        "{}",
        refused.message
    );
    assert_eq!(logged.events()[0]["ceiling_usd"], json!(0.01));
    // Without a ceiling only the scope refuses.
    let unlimited = Logged::new(None);
    let refused = unlimited
        .ledger
        .reserve("scene.draw#1/i.1".into(), usd("0.03"), &scope)
        .unwrap_err();
    assert!(
        refused
            .message
            .starts_with("step budget of scene reached: ")
    );
    assert!(
        unlimited
            .ledger
            .reserve("x#1/i.2".into(), usd("1000000"), &none())
            .is_ok()
    );
}

#[test]
fn a_hold_is_settled_once() {
    let logged = Logged::new(None);
    let ledger = &logged.ledger;
    let hold = ledger
        .reserve("a#1/i.1".into(), usd("0.04"), &none())
        .unwrap();
    ledger.settle(hold.clone(), None);
    assert_eq!(ledger.settle(hold.clone(), None), Usd::ZERO);
    assert_eq!(ledger.settle(hold, Some(Usd::ZERO)), Usd::ZERO);
    assert_eq!(ledger.charged(), usd("0.04"));
    assert_eq!(logged.events().len(), 2);
}

#[test]
fn a_hold_id_is_held_once() {
    let logged = Logged::new(None);
    let ledger = &logged.ledger;
    ledger
        .reserve("a#1/i.1".into(), usd("0.04"), &none())
        .unwrap();
    let refused = ledger
        .reserve("a#1/i.1".into(), usd("0.01"), &none())
        .unwrap_err();
    assert_eq!(refused.message, "a#1/i.1 already holds a reservation");
    assert_eq!(ledger.held(), usd("0.04"));
    assert_eq!(logged.events().len(), 1);
}

#[test]
fn without_a_log_nothing_is_written() {
    let ledger = Ledger::new(Some(usd("0.05")), None);
    let hold = ledger
        .reserve("a#1/i.1".into(), usd("0.04"), &none())
        .unwrap();
    assert!(
        ledger
            .reserve("b#1/i.2".into(), usd("0.04"), &none())
            .is_err()
    );
    assert_eq!(ledger.settle(hold, None), usd("0.04"));
    assert_eq!(ledger.charged(), usd("0.04"));
}

#[test]
fn replay_charges_settlements_and_every_open_hold() {
    let prior = vec![
        json!({"event": "run_started", "charged_usd": 9}),
        json!({"event": "budget_reserved", "node_id": "a#1/old.1", "amount_usd": 0.04}),
        json!({"event": "budget_settled", "node_id": "a#1/old.1", "charged_usd": 0.01, "reported": true}),
        json!({"event": "budget_reserved", "node_id": "b#1/old.2", "amount_usd": 0.02}),
        json!({"event": "budget_settled", "node_id": "b#1/old.2", "charged_usd": 0.02, "reported": false}),
        // Never settled: its process died with the call in flight.
        json!({"event": "budget_reserved", "node_id": "c#1/old.3", "amount_usd": 0.03}),
        json!({"event": "budget_refused", "node_id": "d#1/old.4", "needed_usd": 5, "remaining_usd": 0, "ceiling_usd": 0.1}),
        json!({"event": "call", "id": "a#1", "cost_usd": 7}),
        // Amounts that are not non-negative numbers read as 0.
        json!({"event": "budget_reserved", "node_id": "e#1/old.5", "amount_usd": "0.5"}),
        json!({"event": "budget_settled", "node_id": "f#1/old.6", "charged_usd": -1}),
        json!({"event": "budget_settled", "charged_usd": 3}),
    ];
    let logged = Logged::new(Some("0.10"));
    logged.ledger.replay(&prior, &|_| Vec::new());
    assert_eq!(logged.ledger.charged(), usd("0.06"));
    assert_eq!(logged.ledger.held(), Usd::ZERO);
    // Replay writes nothing.
    assert!(logged.events().is_empty());
    // The ceiling covers the whole folder.
    let refused = logged
        .ledger
        .reserve("g#1/new.1".into(), usd("0.05"), &none())
        .unwrap_err();
    assert_eq!(refused.remaining, usd("0.04"));
}

#[test]
fn replay_restores_step_budget_spend() {
    let prior = vec![
        json!({"event": "budget_reserved", "node_id": "scene['a'].draw#1/old.1", "amount_usd": 0.04}),
        json!({"event": "budget_settled", "node_id": "scene['a'].draw#1/old.1", "charged_usd": 0.02, "reported": true}),
        json!({"event": "budget_reserved", "node_id": "scene['a'].fix#1/old.2", "amount_usd": 0.01}),
        json!({"event": "budget_reserved", "node_id": "scene['b'].draw#1/old.3", "amount_usd": 0.04}),
        json!({"event": "budget_settled", "node_id": "scene['b'].draw#1/old.3", "charged_usd": 0.04, "reported": true}),
    ];
    let scopes_of = |instance: &str| -> Scopes {
        let owner = instance.split('.').next().unwrap_or_default().to_string();
        vec![(owner, usd("0.05"))]
    };
    let seen = std::sync::Mutex::new(Vec::new());
    let recording = |instance: &str| -> Scopes {
        seen.lock().unwrap().push(instance.to_string());
        scopes_of(instance)
    };
    let ledger = Ledger::new(None, None);
    ledger.replay(&prior, &recording);
    let mut asked = seen.into_inner().unwrap();
    asked.sort();
    assert_eq!(
        asked,
        ["scene['a'].draw#1", "scene['a'].fix#1", "scene['b'].draw#1"]
    );
    assert_eq!(ledger.charged(), usd("0.07"));
    // scene['a'] spent 0.02 settled plus 0.01 left open: 0.02 is left.
    let refused = ledger
        .reserve(
            "scene['a'].draw#1/new.1".into(),
            usd("0.03"),
            &scopes_of("scene['a'].draw#1"),
        )
        .unwrap_err();
    assert_eq!(refused.remaining, usd("0.02"));
    assert!(
        ledger
            .reserve(
                "scene['a'].draw#1/new.2".into(),
                usd("0.02"),
                &scopes_of("scene['a'].draw#1")
            )
            .is_ok()
    );
    let refused = ledger
        .reserve(
            "scene['b'].draw#1/new.3".into(),
            usd("0.02"),
            &scopes_of("scene['b'].draw#1"),
        )
        .unwrap_err();
    assert_eq!(refused.remaining, usd("0.01"));
}

#[test]
fn concurrent_reservations_never_pass_the_ceiling() {
    let ledger = Arc::new(Ledger::new(Some(usd("1")), None));
    let threads: Vec<_> = (0..8)
        .map(|t| {
            let ledger = Arc::clone(&ledger);
            std::thread::spawn(move || {
                let mut granted = Vec::new();
                for n in 0..100 {
                    if let Ok(hold) =
                        ledger.reserve(format!("t{t}#1/i.{n}"), usd("0.03"), &Vec::new())
                    {
                        granted.push(hold);
                    }
                }
                granted
            })
        })
        .collect();
    let granted: Vec<Hold> = threads
        .into_iter()
        .flat_map(|thread| thread.join().unwrap())
        .collect();
    // 33 holds of $0.03 fit under $1.
    assert_eq!(granted.len(), 33);
    assert_eq!(ledger.held(), usd("0.99"));
    for hold in granted {
        ledger.settle(hold, None);
    }
    assert_eq!(ledger.charged(), usd("0.99"));
    assert_eq!(ledger.held(), Usd::ZERO);
}
