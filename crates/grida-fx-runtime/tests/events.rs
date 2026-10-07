//! The run record (spec/store.md §8; fx-run-events-v1): event lines, the envelope, reading a log
//! back (torn tails, corrupt lines) and value encoding.

use grida_fx_core::money::Usd;
use grida_fx_core::val::{Collection, FileValue, Val};
use grida_fx_core::value::canon;
use grida_fx_runtime::events::{
    EVENTS_KIND, Event, EventLog, NodeDisplay, decode, encode, new_invocation_id, read_events,
    read_events_tolerant,
};
use grida_fx_runtime::store::Store;
use indexmap::IndexMap;
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;

const PLAN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const DIGEST: &str = "c3f9c8c283a2b1f2f1896f27a01cbe3cddc0c9d93f752e4639035a0f5b36f6e8";
const CALL: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn lines(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn every_event() -> Vec<Event> {
    let encoded_file = json!({"file": {"digest": DIGEST, "kind": "text/plain", "name": "loud['ada']/text", "size": 8}});
    vec![
        Event::RunStarted {
            workflow: "case".into(),
            resumed: false,
            ceiling_usd: None,
            charged_usd: Usd::ZERO,
            estimate_low: Usd(10_000),
            estimate_high: Usd(40_000),
            stand_in: false,
        },
        Event::RunStarted {
            workflow: "case".into(),
            resumed: true,
            ceiling_usd: Some(Usd(1_500_000)),
            charged_usd: Usd(40_000),
            estimate_low: Usd::ZERO,
            estimate_high: Usd::ZERO,
            stand_in: true,
        },
        Event::PhasePlanned {
            phase: 1,
            steps: 3,
            high_usd: Usd::ZERO,
        },
        Event::NodeStarted {
            id: "loud['ada']#1".into(),
            path: "loud['ada']".into(),
            step: "loud".into(),
            take: vec![1, 2],
            identity: None,
            uses: "./nodes/cases.py#shout".into(),
            reads: vec!["count#1".into()],
            ports: json!({"inputs": {}, "params": {}, "outputs": {}}),
            bindings: Vec::new(),
            needs: Vec::new(),
            judges: None,
            routes: IndexMap::from([("image.generate".to_string(), "img-a@acme".to_string())]),
            with: IndexMap::from([
                ("text".to_string(), json!({"value": "ada"})),
                ("brief".to_string(), encoded_file.clone()),
            ]),
        },
        Event::NodeStarted {
            id: "a#1".into(),
            path: "a".into(),
            step: "a".into(),
            take: vec![1],
            identity: Some(DIGEST.into()),
            uses: "fx/image.generate@1".into(),
            reads: vec![],
            ports: json!({"inputs": {}, "params": {}, "outputs": {}}),
            bindings: Vec::new(),
            needs: Vec::new(),
            judges: None,
            routes: IndexMap::new(),
            with: IndexMap::new(),
        },
        Event::NodeFinished {
            id: "a#1".into(),
            path: "a".into(),
            cache_hit: true,
            outputs: IndexMap::from([("text".to_string(), encoded_file)]),
            facts: IndexMap::from([("cost_usd".to_string(), Value::Null)]),
            duration_ms: 12,
        },
        Event::NodeFinished {
            id: "b#1".into(),
            path: "b".into(),
            cache_hit: false,
            outputs: IndexMap::new(),
            facts: IndexMap::new(),
            duration_ms: 0,
        },
        Event::NodeFailed {
            id: "a#1".into(),
            path: "a".into(),
            error: Some("refused on purpose".into()),
            code: Some("node_failure".into()),
            facts: Some(IndexMap::from([("seen".to_string(), json!("x"))])),
            duration_ms: Some(5),
            display: None,
        },
        Event::NodeFailed {
            id: "a#1".into(),
            path: "a".into(),
            error: Some("an assertion failed".into()),
            code: None,
            facts: None,
            duration_ms: None,
            display: None,
        },
        Event::NodeSkipped {
            id: "c#1".into(),
            path: "c".into(),
            reason: Some("something it reads failed".into()),
            blocked: true,
            error: None,
            facts: None,
            duration_ms: None,
            display: None,
        },
        Event::NodeSkipped {
            id: "pick#1".into(),
            path: "pick".into(),
            reason: None,
            blocked: false,
            error: None,
            facts: Some(IndexMap::new()),
            duration_ms: Some(1),
            display: None,
        },
        Event::NodeRetry {
            id: "a#1".into(),
            attempt: 1,
            error: "kaboom".into(),
        },
        Event::Call {
            id: "a#1".into(),
            capability: "image.generate".into(),
            route: "img-a@acme".into(),
            call: CALL.into(),
            cached: true,
            cost_usd: Some(Usd::ZERO),
            stand_in: false,
        },
        Event::Call {
            id: "a#1".into(),
            capability: "image.generate".into(),
            route: "img-a@acme".into(),
            call: CALL.into(),
            cached: false,
            cost_usd: None,
            stand_in: false,
        },
        Event::BudgetReserved {
            node_id: "a#1/0123456789abcdef.1".into(),
            amount_usd: Usd::ZERO,
        },
        Event::BudgetSettled {
            node_id: "a#1/0123456789abcdef.1".into(),
            charged_usd: Usd(40_000),
            reported: false,
        },
        Event::BudgetRefused {
            node_id: "a#1/0123456789abcdef.2".into(),
            needed_usd: Usd(40_000),
            remaining_usd: Usd(10_000),
            ceiling_usd: Some(Usd(50_000)),
        },
        Event::Problem {
            where_: "steps.a".into(),
            message: "never ran: it waits on b#1".into(),
        },
        Event::RunFinished {
            ok: false,
            incomplete: true,
            stopped: None,
            charged_usd: Usd(40_000),
            failed: vec!["a#1".into()],
            outputs: IndexMap::from([
                ("bad".to_string(), json!({"value": {"failed": "a#1"}})),
                ("none".to_string(), json!({"none": true})),
            ]),
        },
        Event::RunFinished {
            ok: false,
            incomplete: true,
            stopped: Some("phase 2 may cost up to $1.00".into()),
            charged_usd: Usd::ZERO,
            failed: vec![],
            outputs: IndexMap::new(),
        },
        Event::RunCancelled {
            reason: "interrupted".into(),
            charged_usd: Usd(1),
        },
        Event::Call {
            id: "a#1".into(),
            capability: "image.generate".into(),
            route: "img-a@acme".into(),
            call: CALL.into(),
            cached: false,
            cost_usd: Some(Usd::ZERO),
            stand_in: true,
        },
        Event::ScopesUpdated {
            scopes: json!([]),
            node_interface_bindings: json!({"leaf#1": []}),
        },
    ]
}

#[test]
fn a_line_is_the_canonical_event_and_a_line_feed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("events.jsonl");
    let log = EventLog::open(&path, "0123456789abcdef", PLAN).unwrap();
    assert_eq!(log.invocation_id(), "0123456789abcdef");
    assert_eq!(log.plan(), PLAN);
    log.emit(&Event::BudgetSettled {
        node_id: "a#1/0123456789abcdef.1".into(),
        charged_usd: Usd(40_000),
        reported: true,
    })
    .unwrap();
    log.emit(&Event::Problem {
        where_: "steps.é".into(),
        message: "line\nbreak \"quoted\"".into(),
    })
    .unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    let written: Vec<&str> = text.split_inclusive('\n').collect();
    assert_eq!(written.len(), 2);
    let offsets: Vec<u64> = lines(&path)
        .iter()
        .map(|e| e["offset_ms"].as_u64().unwrap())
        .collect();
    assert_eq!(
        written[0],
        format!(
            "{{\"charged_usd\":0.04,\"event\":\"budget_settled\",\"invocation_id\":\"0123456789abcdef\",\
             \"kind\":\"fx-run-events-v1\",\"node_id\":\"a#1/0123456789abcdef.1\",\"offset_ms\":{},\
             \"plan\":\"{PLAN}\",\"reported\":true}}\n",
            offsets[0]
        )
    );
    assert_eq!(
        written[1],
        format!(
            "{{\"event\":\"problem\",\"invocation_id\":\"0123456789abcdef\",\"kind\":\"fx-run-events-v1\",\
             \"message\":\"line\\nbreak \\\"quoted\\\"\",\"offset_ms\":{},\"plan\":\"{PLAN}\",\
             \"where\":\"steps.é\"}}\n",
            offsets[1]
        )
    );
    for line in &written {
        let value: Value = serde_json::from_str(line).unwrap();
        assert_eq!(format!("{}\n", canon(&value)), *line);
    }
    assert!(offsets[0] <= offsets[1]);
}

#[test]
fn every_event_matches_the_schema() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("events.jsonl");
    let log = EventLog::open(&path, &new_invocation_id(), PLAN).unwrap();
    let events = every_event();
    for event in &events {
        log.emit(event).unwrap();
    }
    let schema: Value = serde_json::from_str(include_str!(
        "../../../spec/schemas/fx-run-events-v1.schema.json"
    ))
    .unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    let written = lines(&path);
    assert_eq!(written.len(), events.len());
    for (event, line) in events.iter().zip(&written) {
        let errors: Vec<String> = validator.iter_errors(line).map(|e| e.to_string()).collect();
        assert!(errors.is_empty(), "{line}: {errors:#?}");
        assert_eq!(line["kind"], EVENTS_KIND);
        assert_eq!(line["event"], event.name());
        assert_eq!(line["plan"], PLAN);
        assert_eq!(line["invocation_id"], log.invocation_id());
        // The envelope plus the event's own members, nothing else.
        let mut expected: Vec<String> = event.to_fields().keys().cloned().collect();
        expected.extend(["kind", "event", "invocation_id", "plan", "offset_ms"].map(String::from));
        expected.sort();
        let mut found: Vec<String> = line.as_object().unwrap().keys().cloned().collect();
        found.sort();
        assert_eq!(found, expected);
    }
}

#[test]
fn terminal_display_metadata_is_flattened_without_execution_values() {
    let display = NodeDisplay {
        uses: "./nodes/cases.py#shout".into(),
        reads: vec!["source#1".into()],
        ports: json!({"inputs":{"image":"image"},"outputs":{"report":"json"},"params":{"label":{"type":"string"}}}),
        bindings: vec![grida_fx_core::expand::wiring::Binding {
            source: "source#1".into(),
            source_port: "image".into(),
            target_port: "image".into(),
            source_kind: grida_fx_core::expand::wiring::SourceKind::Output,
        }],
        needs: vec!["barrier#1".into()],
        judges: None,
    };
    let events = [
        Event::NodeFailed {
            id: "dynamic['one']#1".into(),
            path: "dynamic['one']".into(),
            error: Some("assertion failed".into()),
            code: None,
            facts: None,
            duration_ms: None,
            display: Some(display.clone()),
        },
        Event::NodeSkipped {
            id: "dynamic['two']#1".into(),
            path: "dynamic['two']".into(),
            reason: Some("source failed".into()),
            blocked: true,
            error: None,
            facts: None,
            duration_ms: None,
            display: Some(display.clone()),
        },
    ];
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("events.jsonl");
    let log = EventLog::open(&path, "terminal-metadata", PLAN).unwrap();
    let schema: Value = serde_json::from_str(include_str!(
        "../../../spec/schemas/fx-run-events-v1.schema.json"
    ))
    .unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    for event in &events {
        log.emit(event).unwrap();
    }
    let written = lines(&path);
    assert_eq!(written.len(), 2);
    for (event, line) in events.iter().zip(&written) {
        let errors: Vec<_> = validator
            .iter_errors(line)
            .map(|error| error.to_string())
            .collect();
        assert!(errors.is_empty(), "{errors:?}");
        let fields = event.to_fields();
        assert_eq!(fields["uses"], display.uses);
        assert_eq!(fields["reads"], json!(["source#1"]));
        assert_eq!(fields["ports"], display.ports);
        assert_eq!(
            fields["bindings"],
            json!([{"source":"source#1","source_port":"image","target_port":"image","source_kind":"output"}])
        );
        assert_eq!(fields["needs"], json!(["barrier#1"]));
        assert_eq!(fields["judges"], Value::Null);
        assert!(!fields.contains_key("display"));
        assert!(!fields.contains_key("with"));
        assert!(!fields.contains_key("identity"));
        assert_ne!(line["event"], "node_started");
    }
}

#[test]
fn members_follow_the_schema_names() {
    let fields = |event: &Event| Value::Object(event.to_fields());
    let events = every_event();
    assert_eq!(
        fields(&events[0]),
        json!({"workflow": "case", "resumed": false, "ceiling_usd": null, "charged_usd": 0,
               "estimate": {"low_usd": 0.01, "high_usd": 0.04}})
    );
    assert_eq!(fields(&events[1])["ceiling_usd"], json!(1.5));
    // `stand_in` only when true.
    assert_eq!(fields(&events[1])["stand_in"], json!(true));
    assert_eq!(fields(&events[3])["take"], json!([1, 2]));
    assert_eq!(fields(&events[3])["identity"], Value::Null);
    assert_eq!(
        fields(&events[3])["routes"],
        json!({"image.generate": "img-a@acme"})
    );
    assert_eq!(fields(&events[4])["identity"], DIGEST);
    assert_eq!(fields(&events[5])["cache"], "hit");
    assert_eq!(fields(&events[6])["cache"], "miss");
    assert_eq!(
        fields(&events[7]),
        json!({"id": "a#1", "path": "a", "error": "refused on purpose", "code": "node_failure",
               "facts": {"seen": "x"}, "duration_ms": 5})
    );
    assert_eq!(
        fields(&events[8]),
        json!({"id": "a#1", "path": "a", "error": "an assertion failed", "code": null})
    );
    assert_eq!(
        fields(&events[9]),
        json!({"id": "c#1", "path": "c", "reason": "something it reads failed", "blocked": true})
    );
    assert_eq!(
        fields(&events[10]),
        json!({"id": "pick#1", "path": "pick", "error": null, "facts": {}, "duration_ms": 1})
    );
    assert_eq!(fields(&events[12])["cost_usd"], json!(0));
    assert_eq!(fields(&events[13])["cost_usd"], Value::Null);
    assert_eq!(
        fields(&events[14]),
        json!({"node_id": "a#1/0123456789abcdef.1", "amount_usd": 0})
    );
    assert_eq!(
        fields(&events[16]),
        json!({"node_id": "a#1/0123456789abcdef.2", "needed_usd": 0.04, "remaining_usd": 0.01, "ceiling_usd": 0.05})
    );
    assert_eq!(
        fields(&events[17]),
        json!({"where": "steps.a", "message": "never ran: it waits on b#1"})
    );
    assert_eq!(fields(&events[18])["stopped"], Value::Null);
    assert_eq!(fields(&events[18])["failed"], json!(["a#1"]));
    assert_eq!(
        fields(&events[20]),
        json!({"reason": "interrupted", "charged_usd": 0.000001})
    );
    assert!(
        !fields(&events[12])
            .as_object()
            .unwrap()
            .contains_key("stand_in")
    );
    assert_eq!(
        fields(&events[21]),
        json!({"id": "a#1", "capability": "image.generate", "route": "img-a@acme", "call": CALL,
               "cached": false, "cost_usd": 0, "stand_in": true})
    );
}

#[test]
fn a_resumed_log_appends() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("events.jsonl");
    let problem = Event::Problem {
        where_: "w".into(),
        message: "m".into(),
    };
    EventLog::open(&path, "1111111111111111", PLAN)
        .unwrap()
        .emit(&problem)
        .unwrap();
    EventLog::open(&path, "2222222222222222", PLAN)
        .unwrap()
        .emit(&problem)
        .unwrap();
    let ids: Vec<Value> = read_events(&path)
        .unwrap()
        .iter()
        .map(|e| e["invocation_id"].clone())
        .collect();
    assert_eq!(ids, [json!("1111111111111111"), json!("2222222222222222")]);
}

#[cfg(unix)]
#[test]
fn the_log_is_readable_by_its_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("events.jsonl");
    EventLog::open(&path, "1111111111111111", PLAN).unwrap();
    let mode = std::fs::metadata(&path).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn concurrent_emits_write_whole_lines() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("events.jsonl");
    let log = Arc::new(EventLog::open(&path, "1111111111111111", PLAN).unwrap());
    let threads: Vec<_> = (0..8)
        .map(|t| {
            let log = Arc::clone(&log);
            std::thread::spawn(move || {
                for n in 0..50 {
                    log.emit(&Event::NodeRetry {
                        id: format!("t{t}#1"),
                        attempt: n + 1,
                        error: "x".repeat(300),
                    })
                    .unwrap();
                }
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
    let events = read_events(&path).unwrap();
    assert_eq!(events.len(), 400);
    // Each task's own events stay in order.
    for t in 0..8 {
        let attempts: Vec<u64> = events
            .iter()
            .filter(|e| e["id"] == format!("t{t}#1"))
            .map(|e| e["attempt"].as_u64().unwrap())
            .collect();
        assert_eq!(attempts, (1..=50).collect::<Vec<_>>());
    }
}

#[test]
fn invocation_ids_are_new_each_time() {
    let ids: std::collections::BTreeSet<String> = (0..100).map(|_| new_invocation_id()).collect();
    assert_eq!(ids.len(), 100);
    for id in &ids {
        assert_eq!(id.len(), 16);
        assert!(
            id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')),
            "{id}"
        );
    }
}

// ------------------------------------------------------------------ reading

const ONE: &str = "{\"event\":\"run_started\",\"n\":1}\n";
const TWO: &str = "{\"event\":\"run_finished\",\"n\":2}\n";

#[test]
fn a_missing_log_is_empty() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("events.jsonl");
    assert_eq!(read_events(&path), Ok(vec![]));
    assert!(read_events_tolerant(&path).unwrap().is_empty());
    assert!(!path.exists());
}

#[test]
fn a_torn_tail_is_cut_from_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("events.jsonl");
    let torn = format!("{ONE}{TWO}{{\"event\":\"node_sta");
    std::fs::write(&path, &torn).unwrap();
    let events = read_events(&path).unwrap();
    assert_eq!(
        events,
        [
            json!({"event": "run_started", "n": 1}),
            json!({"event": "run_finished", "n": 2})
        ]
    );
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        format!("{ONE}{TWO}")
    );
    // A complete object without its line feed is torn too.
    std::fs::write(&path, format!("{ONE}{{\"event\":\"x\"}}")).unwrap();
    assert_eq!(read_events(&path).unwrap().len(), 1);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), ONE);
    // A log with no whole line is emptied.
    std::fs::write(&path, "{\"ev").unwrap();
    assert_eq!(read_events(&path), Ok(vec![]));
    assert_eq!(std::fs::read(&path).unwrap(), b"");
    // The repaired log takes new lines where the torn one was.
    std::fs::write(&path, format!("{ONE}{{\"torn")).unwrap();
    read_events(&path).unwrap();
    EventLog::open(&path, "1111111111111111", PLAN)
        .unwrap()
        .emit(&Event::Problem {
            where_: "w".into(),
            message: "m".into(),
        })
        .unwrap();
    let events = read_events(&path).unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[1]["event"], "problem");
}

#[test]
fn blank_lines_are_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("events.jsonl");
    std::fs::write(&path, format!("\n{ONE}  \n\n{TWO}")).unwrap();
    assert_eq!(read_events(&path).unwrap().len(), 2);
    assert_eq!(read_events_tolerant(&path).unwrap().len(), 2);
}

#[test]
fn a_corrupt_line_is_an_error_naming_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("events.jsonl");
    for (bad, line) in [
        ("{not json}\n", 2),
        ("[1, 2]\n", 2),
        ("\"text\"\n", 2),
        ("{\"a\": 1, \"a\": 2}\n", 2),
        ("{\"a\": 1} trailing\n", 2),
        ("\n{\"x\": \"\\ud800\"}\n", 3),
    ] {
        let text = format!("{ONE}{bad}{TWO}");
        std::fs::write(&path, &text).unwrap();
        assert_eq!(
            read_events(&path),
            Err(format!("events.jsonl line {line} is not an event")),
            "{bad:?}"
        );
        // Nothing is repaired or written.
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    }
    std::fs::write(&path, [ONE.as_bytes(), b"{\"a\": \"\xff\"}\n"].concat()).unwrap();
    assert_eq!(
        read_events(&path),
        Err("events.jsonl line 2 is not an event".to_string())
    );
}

#[test]
fn the_tolerant_reader_stops_at_the_first_unreadable_line_and_never_writes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("events.jsonl");
    let text = format!("{ONE}{{broken\n{TWO}");
    std::fs::write(&path, &text).unwrap();
    assert_eq!(
        read_events_tolerant(&path).unwrap(),
        [json!({"event": "run_started", "n": 1})]
    );
    let torn = format!("{ONE}{TWO}{{\"event\":\"node");
    std::fs::write(&path, &torn).unwrap();
    assert_eq!(read_events_tolerant(&path).unwrap().len(), 2);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), torn);
}

// ------------------------------------------------------------------ encoding

fn text_file(key: Option<&str>) -> Val {
    Val::File(Box::new(FileValue {
        digest: DIGEST.into(),
        kind: "text/plain".into(),
        name: "loud['ada']/text".into(),
        size: 8,
        key: key.map(str::to_string),
        content: None,
        location: None,
    }))
}

#[test]
fn values_without_files_decode_without_the_store() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path());
    let values = [
        Val::Null,
        Val::Number(3.0),
        Val::Number(0.5),
        Val::Str("ada".into()),
        Val::Bool(false),
        Val::List(vec![Val::Str("a".into()), Val::Null]),
        Val::Object(IndexMap::from([("a".to_string(), Val::Number(1.0))])),
        Val::Collection(Box::new(Collection {
            items: vec![
                ("ada".into(), Val::Str("x".into())),
                ("bo".into(), Val::List(vec![])),
            ],
            verdicts: IndexMap::new(),
        })),
    ];
    for value in values {
        let encoded = encode(&value);
        assert_eq!(decode(&encoded, &store), Ok(value.clone()), "{encoded}");
    }
    // Missing encodes as none and reads back as null.
    assert_eq!(decode(&encode(&Val::Missing), &store), Ok(Val::Null));
    // A failed upstream result is a plain value in an event.
    assert_eq!(
        decode(&encode(&Val::Failed("a#1".into())), &store),
        Ok(Val::Object(IndexMap::from([(
            "failed".to_string(),
            Val::Str("a#1".into())
        )])))
    );
}

#[test]
fn what_is_not_an_encoded_value_does_not_decode() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path());
    for bad in [
        json!(1),
        json!({}),
        json!({"value": 1, "list": []}),
        json!({"other": 1}),
        json!({"none": false}),
        json!({"list": {}}),
        json!({"collection": [["a"]]}),
        json!({"collection": [[1, {"none": true}]]}),
        json!({"list": [{"bad": 1}]}),
        json!({"file": {"digest": "nope", "kind": "json", "name": "x", "size": 1}}),
    ] {
        assert!(decode(&bad, &store).is_err(), "{bad}");
    }
    // A file the store does not hold does not decode.
    let missing = encode(&text_file(None));
    assert!(decode(&missing, &store).unwrap_err().contains(DIGEST));
}

#[test]
fn files_round_trip_through_the_store() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path());
    let stored = store.put_bytes(b"one\ntwo\n").unwrap();
    assert_eq!(stored.digest, DIGEST);
    let value = Val::Collection(Box::new(Collection {
        items: vec![
            ("ada".into(), text_file(Some("ada"))),
            ("bo".into(), Val::List(vec![text_file(None), Val::Null])),
        ],
        verdicts: IndexMap::new(),
    }));
    let encoded = encode(&value);
    assert_eq!(encoded["collection"][0][1]["file"]["key"], "ada");
    let decoded = decode(&encoded, &store).unwrap();
    assert_eq!(decoded, value);
    let Val::Collection(collection) = &decoded else {
        panic!("{decoded:?}");
    };
    let Val::File(file) = &collection.items[0].1 else {
        panic!("{decoded:?}");
    };
    assert_eq!(
        file.location.as_deref(),
        Some(store.file_path(DIGEST).unwrap().as_path())
    );
    // A text file's content is read as expansion reads step outputs.
    assert!(file.content.is_some());
}
