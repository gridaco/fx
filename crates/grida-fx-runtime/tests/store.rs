//! The store (spec/store.md §1–§7): layout, atomic writes, trust, records, read sets.

use grida_fx_core::expand::{Instance, State};
use grida_fx_core::host::ResultCache;
use grida_fx_core::money::Usd;
use grida_fx_core::registry::{ResolvedType, TypeOrigin};
use grida_fx_core::spec::{BodyKind, NodeSpec, Retry};
use grida_fx_core::val::{Collection, FileContent, FileValue, Val};
use grida_fx_core::value::{canon, file_digest};
use grida_fx_runtime::store::records::{
    CALL_KIND, CallRecord, FileEntry, JOB_KIND, JobRecord, JobState, RESULT_KIND, RecordOutput,
    ResultRecord, RouteEntry, call_key,
};
use grida_fx_runtime::store::{ReadSet, Store, StoreError, atomic_write, files_in, read_set};
use indexmap::IndexMap;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use tempfile::TempDir;

/// `one\ntwo\n` (spec/identity.md §14, "A file").
const ONE_TWO: &[u8] = b"one\ntwo\n";
const ONE_TWO_DIGEST: &str = "c3f9c8c283a2b1f2f1896f27a01cbe3cddc0c9d93f752e4639035a0f5b36f6e8";
/// `img-a@acme` serving `image.generate` (spec/identity.md §14, "A route").
const FINGERPRINT: &str = "4d41b81c56215efdd18574eab8e2b704a8ecc86af08d569f55cd29973c4e7ed4";
/// The call of conformance/cache-replay.
const REPLAY_KEY: &str = "3aa41bf6e466138e920882b87c9b7ef9fc22dc245a2861c121f80adaaafd066d";
const REPLAY_IMAGE: &str = "f3945de0c1182a1b279816f51ef2e79938d04957c7bfde94cd4bf2eb4c2170b4";

fn store() -> (TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("cache"));
    (dir, store)
}

fn digest_of(n: u8) -> String {
    format!("{n:02x}").repeat(32)
}

/// A file value of these bytes, not in any store.
fn file(bytes: &[u8], kind: &str, name: &str) -> FileValue {
    FileValue {
        digest: file_digest(bytes),
        kind: kind.into(),
        name: name.into(),
        size: bytes.len() as u64,
        key: None,
        content: None,
        location: None,
    }
}

fn entry(bytes: &[u8], kind: &str, name: &str) -> FileEntry {
    FileEntry {
        digest: file_digest(bytes),
        kind: kind.into(),
        name: name.into(),
        size: bytes.len() as u64,
        key: None,
    }
}

/// Every name in a folder, sorted.
fn names(folder: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(folder)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// Every file under a folder, as paths relative to it.
fn tree(folder: &Path) -> Vec<String> {
    fn walk(base: &Path, folder: &Path, out: &mut Vec<String>) {
        for entry in fs::read_dir(folder).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(base, &path, out);
            } else {
                let relative = path.strip_prefix(base).unwrap();
                out.push(relative.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    let mut out = Vec::new();
    if folder.exists() {
        walk(folder, folder, &mut out);
    }
    out.sort();
    out
}

#[allow(clippy::permissions_set_readonly_false)]
fn make_writable(path: &Path) {
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_readonly(false);
    fs::set_permissions(path, permissions).unwrap();
}

/// A result record whose outputs hold one stored file.
fn result_record(store: &Store, identity: &str, read: ReadSet) -> ResultRecord {
    store.put_bytes(ONE_TWO).unwrap();
    ResultRecord {
        identity: identity.into(),
        type_identity: "nodes/cases.py#lines@1".into(),
        outputs: IndexMap::from([(
            "lines".to_string(),
            RecordOutput::File(entry(ONE_TWO, "text/plain", "count/lines")),
        )]),
        facts: IndexMap::from([("count".to_string(), json!(2))]),
        read,
        cost_usd: None,
    }
}

/// The call record of conformance/cache-replay, built by hand.
fn replay_call(image: &[u8]) -> CallRecord {
    CallRecord {
        key: REPLAY_KEY.into(),
        capability: "image.generate".into(),
        route: RouteEntry {
            id: "img-a@acme".into(),
            fingerprint: FINGERPRINT.into(),
        },
        request: json!({"background": "auto", "prompt": "a lantern"}),
        take: vec![1],
        files: IndexMap::from([(
            "image".to_string(),
            FileEntry {
                digest: file_digest(image),
                kind: "image/png".into(),
                name: "image".into(),
                size: image.len() as u64,
                key: None,
            },
        )]),
        data: Value::Null,
        cost_usd: Some(Usd(20_000)),
    }
}

fn job(key: &str, state: JobState, handle: Option<Value>) -> JobRecord {
    JobRecord {
        key: key.into(),
        capability: "video.generate".into(),
        route: RouteEntry {
            id: "vid-a@acme".into(),
            fingerprint: FINGERPRINT.into(),
        },
        request: json!({"prompt": "a lantern at dusk"}),
        take: vec![2, 1],
        state,
        handle,
    }
}

fn replay_cache() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../conformance/cache-replay/in/.fx/cache")
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let path = entry.unwrap().path();
        let target = to.join(path.file_name().unwrap());
        if path.is_dir() {
            copy_tree(&path, &target);
        } else {
            fs::copy(&path, &target).unwrap();
        }
    }
}

fn instance(identity: Option<&str>, with: IndexMap<String, Val>) -> Instance {
    let spec = NodeSpec {
        name: "lines".into(),
        description: None,
        inputs: IndexMap::new(),
        params: IndexMap::new(),
        outputs: IndexMap::new(),
        judge: false,
        capability: None,
        calls: IndexMap::new(),
        resources: Vec::new(),
        tools: Vec::new(),
        view: None,
        version: Some(1),
        retry: Retry::Service,
    };
    Instance {
        id: "count#1".into(),
        path: "count".into(),
        step: "count".into(),
        takes: vec![1],
        uses: "./nodes/cases.py#lines".into(),
        ty: Rc::new(ResolvedType {
            uses: "./nodes/cases.py#lines".into(),
            identity: "nodes/cases.py#lines@1".into(),
            spec: Rc::new(spec),
            origin: TypeOrigin::Builtin {
                name: "lines".into(),
                major: 1,
            },
            body: BodyKind::None,
            source: None,
            drift: None,
        }),
        with,
        needs: Vec::new(),
        state: State::Planned,
        identity: identity.map(str::to_string),
        routes: IndexMap::new(),
        prices: Vec::new(),
        phase: 1,
        key: None,
        judges: None,
        judge_policy: None,
        judged_by: Vec::new(),
        view: Value::Bool(false),
        at_plan: false,
        budget: None,
        concurrency_group: None,
        concurrency: None,
        timeout_s: None,
        reason: None,
        reads: BTreeSet::new(),
    }
}

// ------------------------------------------------------------------ layout

#[test]
fn every_store_path_is_built_from_a_digest() {
    let (_dir, store) = store();
    let d = ONE_TWO_DIGEST;
    assert_eq!(
        store.file_path(d).unwrap(),
        store.root().join("files").join("c3").join(d)
    );
    assert_eq!(store.work_root(), store.root().join("work"));
    for bad in [
        "",
        "../escape",
        &d.to_uppercase(),
        &d[..63],
        &format!("{d}0"),
        &format!("g{}", &d[1..]),
    ] {
        let refused = StoreError::NotADigest(bad.to_string());
        assert_eq!(store.file_path(bad), Err(refused.clone()));
        assert_eq!(store.load_job(bad), Err(refused.clone()));
        assert_eq!(store.remove_job(bad), Err(refused.clone()));
        assert_eq!(
            store.save_job(&job(bad, JobState::Settled, None)),
            Err(refused.clone())
        );
        assert_eq!(store.verify(bad), Err(refused.clone()));
        assert!(!store.has(bad, 0));
        assert!(store.load_call(bad).is_none());
        assert!(store.load_result(bad, &ReadSet::new()).is_none());
        let mut record = result_record(&store, d, ReadSet::new());
        record.identity = bad.to_string();
        assert_eq!(store.save_result(&record), Err(refused.clone()));
        let mut call = replay_call(ONE_TWO);
        call.key = bad.to_string();
        assert_eq!(store.save_call(&call), Err(refused.clone()));
        let value = FileEntry {
            digest: bad.to_string(),
            ..entry(ONE_TWO, "text/plain", "x")
        };
        assert_eq!(store.file_value(&value), Err(refused));
    }
    assert_eq!(
        StoreError::NotADigest("../escape".into()).to_string(),
        "not a digest: ../escape"
    );
    // Nothing was written outside files/.
    assert!(tree(store.root()).iter().all(|p| p.starts_with("files/")));
}

#[test]
fn records_and_files_sit_where_store_md_puts_them() {
    let (_dir, store) = store();
    let identity = digest_of(0xab);
    store
        .save_result(&result_record(&store, &identity, ReadSet::new()))
        .unwrap();
    let image = b"\x89PNG fake".to_vec();
    store.put_bytes(&image).unwrap();
    let call = {
        let mut call = replay_call(&image);
        call.request = json!({"prompt": "x"});
        call.key = call.computed_key();
        call
    };
    store.save_call(&call).unwrap();
    let key = digest_of(0xcd);
    store
        .save_job(&job(&key, JobState::Submitting, None))
        .unwrap();
    let image_digest = file_digest(&image);
    let mut expected = vec![
        format!("files/c3/{ONE_TWO_DIGEST}"),
        format!("files/{}/{image_digest}", &image_digest[..2]),
        format!("results/ab/{identity}.json"),
        format!("calls/{}/{}.json", &call.key[..2], call.key),
        format!("jobs/{key}.json"),
    ];
    expected.sort();
    assert_eq!(tree(store.root()), expected);
}

// ------------------------------------------------------------ atomic writes

#[test]
fn an_atomic_write_replaces_and_leaves_no_temporary_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("deep").join("folder").join("record.json");
    atomic_write(&path, b"first", true).unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"first");
    assert!(fs::metadata(&path).unwrap().permissions().readonly());
    // A read-only file is replaced all the same (a rename, never a write into it).
    atomic_write(&path, b"second, longer", false).unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"second, longer");
    assert!(!fs::metadata(&path).unwrap().permissions().readonly());
    assert_eq!(
        names(path.parent().unwrap()),
        vec!["record.json".to_string()]
    );
}

#[test]
fn a_failed_atomic_write_leaves_nothing_behind() {
    let dir = tempfile::tempdir().unwrap();
    let blocked = dir.path().join("blocked");
    fs::create_dir(&blocked).unwrap();
    fs::write(blocked.join("keep"), b"x").unwrap();
    assert!(atomic_write(&blocked, b"bytes", false).is_err());
    assert_eq!(names(dir.path()), vec!["blocked".to_string()]);
    assert_eq!(names(&blocked), vec!["keep".to_string()]);
}

// ------------------------------------------------------------------- files

#[test]
fn bytes_are_stored_once_read_only_under_their_digest() {
    let (_dir, store) = store();
    let stored = store.put_bytes(ONE_TWO).unwrap();
    assert_eq!(stored.digest, ONE_TWO_DIGEST);
    assert_eq!(stored.size, 8);
    let path = store.file_path(ONE_TWO_DIGEST).unwrap();
    assert_eq!(fs::read(&path).unwrap(), ONE_TWO);
    assert!(fs::metadata(&path).unwrap().permissions().readonly());
    assert!(store.has(ONE_TWO_DIGEST, 8));
    assert!(!store.has(ONE_TWO_DIGEST, 9));
    assert!(!store.has(&digest_of(1), 8));
    assert_eq!(store.verify(ONE_TWO_DIGEST), Ok(true));
    assert_eq!(store.verify(&digest_of(1)), Ok(false));
    assert_eq!(
        names(path.parent().unwrap()),
        vec![ONE_TWO_DIGEST.to_string()]
    );
}

#[test]
fn a_present_file_is_not_written_again() {
    let (_dir, store) = store();
    store.put_bytes(ONE_TWO).unwrap();
    let path = store.file_path(ONE_TWO_DIGEST).unwrap();
    // Same size, other bytes: present by size, so neither put rewrites it; a full check sees it.
    make_writable(&path);
    fs::write(&path, b"ONE\nTWO\n").unwrap();
    store.put_bytes(ONE_TWO).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("lines.txt");
    fs::write(&source, ONE_TWO).unwrap();
    assert_eq!(
        store.put_file(&source, "lines.txt").unwrap().digest,
        ONE_TWO_DIGEST
    );
    assert_eq!(fs::read(&path).unwrap(), b"ONE\nTWO\n");
    assert_eq!(store.verify(ONE_TWO_DIGEST), Ok(false));
    // Another size is not present: it is written again.
    fs::write(&path, b"short").unwrap();
    assert!(!store.has(ONE_TWO_DIGEST, 8));
    store.put_bytes(ONE_TWO).unwrap();
    assert_eq!(fs::read(&path).unwrap(), ONE_TWO);
    assert_eq!(store.verify(ONE_TWO_DIGEST), Ok(true));
}

#[test]
fn a_file_is_streamed_into_the_store() {
    let (dir, store) = store();
    let source = dir.path().join("big.bin");
    let bytes: Vec<u8> = (0..200_000u32).map(|i| (i * 7 % 251) as u8).collect();
    fs::write(&source, &bytes).unwrap();
    let stored = store.put_file(&source, "big.bin").unwrap();
    assert_eq!(stored.digest, file_digest(&bytes));
    assert_eq!(stored.size, bytes.len() as u64);
    let path = store.file_path(&stored.digest).unwrap();
    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert!(fs::metadata(&path).unwrap().permissions().readonly());
    assert_eq!(names(path.parent().unwrap()), vec![stored.digest.clone()]);
    // The source is left as it was.
    assert_eq!(fs::read(&source).unwrap(), bytes);
}

#[test]
fn errors_name_the_label_never_an_absolute_path() {
    let (dir, store) = store();
    let missing = dir.path().join("inputs").join("brief.txt");
    let error = store.put_file(&missing, "inputs/brief.txt").unwrap_err();
    let text = error.to_string();
    assert!(text.contains("inputs/brief.txt"), "{text}");
    assert!(text.contains("no such file"), "{text}");
    assert!(!text.contains(&*dir.path().to_string_lossy()), "{text}");
}

#[test]
fn adopting_a_file_copies_it_once_and_points_at_the_store() {
    let (dir, store) = store();
    let source = dir.path().join("brief.txt");
    fs::write(&source, ONE_TWO).unwrap();
    let mut input = file(ONE_TWO, "text/plain", "brief.txt");
    input.location = Some(source.clone());
    input.content = Some(FileContent::Text("one\ntwo\n".into()));
    let adopted = store.adopt(&input).unwrap();
    let copy = store.file_path(ONE_TWO_DIGEST).unwrap();
    assert_eq!(adopted.location.as_deref(), Some(copy.as_path()));
    assert_eq!(adopted, input);
    assert_eq!(adopted.content, input.content);
    assert_eq!(fs::read(&copy).unwrap(), ONE_TWO);
    // Present: the source is not read again (it may be gone).
    fs::remove_file(&source).unwrap();
    let again = store.adopt(&input).unwrap();
    assert_eq!(again.location.as_deref(), Some(copy.as_path()));
    let mut nowhere = input.clone();
    nowhere.location = None;
    assert!(store.adopt(&nowhere).is_ok());
}

#[test]
fn adopting_a_file_that_changed_since_planning_is_refused() {
    let (dir, store) = store();
    let source = dir.path().join("brief.txt");
    fs::write(&source, ONE_TWO).unwrap();
    let mut input = file(ONE_TWO, "text/plain", "brief.txt");
    input.location = Some(source.clone());
    fs::write(&source, b"one\ntwo\nthree\n").unwrap();
    let text = store.adopt(&input).unwrap_err().to_string();
    assert!(
        text.contains("brief.txt changed since it was planned"),
        "{text}"
    );
    assert!(!text.contains(&*dir.path().to_string_lossy()), "{text}");
    let mut lost = file(b"never stored", "file", "lost.bin");
    lost.location = None;
    let text = store.adopt(&lost).unwrap_err().to_string();
    assert!(text.contains("lost.bin has no local copy"), "{text}");
}

#[test]
fn file_values_read_text_and_json_content() {
    let (dir, store) = store();
    let text_bytes = b"\xEF\xBB\xBFHello.\r\n";
    store.put_bytes(text_bytes).unwrap();
    let value = store
        .file_value(&entry(text_bytes, "text/markdown", "note"))
        .unwrap();
    assert_eq!(value.content, Some(FileContent::Text("Hello.\r\n".into())));
    assert_eq!(
        value.location,
        Some(store.file_path(&file_digest(text_bytes)).unwrap())
    );
    assert_eq!(value.digest, file_digest(text_bytes));
    assert_eq!(value.name, "note");
    assert_eq!(value.size, 11);

    let json_bytes = br#"{"b": 1.0, "a": [true, null]}"#;
    store.put_bytes(json_bytes).unwrap();
    for kind in ["json", "annotations", "model/gltf+json"] {
        let mut keyed = entry(json_bytes, kind, "data");
        keyed.key = Some("ada".into());
        let value = store.file_value(&keyed).unwrap();
        assert_eq!(
            value.content,
            Some(FileContent::Json(json!({"b": 1, "a": [true, null]})))
        );
        assert_eq!(value.key.as_deref(), Some("ada"));
    }

    let png = b"\x89PNG\r\n\x1a\n not really";
    store.put_bytes(png).unwrap();
    assert_eq!(
        store
            .file_value(&entry(png, "image/png", "image"))
            .unwrap()
            .content,
        None
    );

    let big = vec![b'a'; 1_000_001];
    store.put_bytes(&big).unwrap();
    assert_eq!(
        store
            .file_value(&entry(&big, "text/plain", "big"))
            .unwrap()
            .content,
        None
    );

    let root = dir.path().to_string_lossy().into_owned();
    for (bytes, kind) in [
        (&b"\xff\xfe not utf-8"[..], "text/plain"),
        (&b"{\"a\": 1, \"a\": 2}"[..], "json"),
        (&b"{\"a\": NaN}"[..], "json"),
        (&b"{\"missing\": true}"[..], "json"),
    ] {
        store.put_bytes(bytes).unwrap();
        let error = store.file_value(&entry(bytes, kind, "bad")).unwrap_err();
        let text = error.to_string();
        assert!(matches!(error, StoreError::Io { .. }), "{text}");
        assert!(text.contains("bad"), "{text}");
        assert!(!text.contains(&root), "{text}");
    }
}

// ------------------------------------------------------------------ records

#[test]
fn records_are_written_canonically_and_read_only() {
    let (_dir, store) = store();
    let identity = digest_of(0x11);
    let read = ReadSet::from([("text/0".to_string(), ONE_TWO_DIGEST.to_string())]);
    let mut record = result_record(&store, &identity, read);
    record.cost_usd = Some(Usd(1_500_000));
    store.save_result(&record).unwrap();
    let path = store
        .root()
        .join("results")
        .join("11")
        .join(format!("{identity}.json"));
    let bytes = fs::read_to_string(&path).unwrap();
    assert_eq!(
        bytes,
        format!(
            concat!(
                r#"{{"cost_usd":1.5,"facts":{{"count":2}},"identity":"{i}","kind":"fx-result-record-v1","#,
                r#""outputs":{{"lines":{{"file":{{"digest":"{d}","kind":"text/plain","name":"count/lines","size":8}}}}}},"#,
                r#""read":{{"text/0":"{d}"}},"type":"nodes/cases.py#lines@1"}}"#
            ),
            i = identity,
            d = ONE_TWO_DIGEST
        )
    );
    assert!(fs::metadata(&path).unwrap().permissions().readonly());
    assert_eq!(
        names(path.parent().unwrap()),
        vec![format!("{identity}.json")]
    );
    // Writing it again replaces it whole.
    store.save_result(&record).unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), bytes);
}

#[test]
fn the_replay_call_record_is_written_byte_for_byte() {
    let (_dir, store) = store();
    let committed = replay_cache();
    let image = fs::read(committed.join("files/f3").join(REPLAY_IMAGE)).unwrap();
    store.put_bytes(&image).unwrap();
    let record = replay_call(&image);
    store.save_call(&record).unwrap();
    let relative = format!("calls/3a/{REPLAY_KEY}.json");
    assert_eq!(
        fs::read(store.root().join(&relative)).unwrap(),
        fs::read(committed.join(&relative)).unwrap()
    );
}

#[test]
fn records_round_trip() {
    let a = entry(b"a", "image/png", "a");
    let mut b = entry(b"b", "image/png", "b");
    b.key = Some("bo".into());
    let result = ResultRecord {
        identity: digest_of(1),
        type_identity: "source:".to_string() + &digest_of(2),
        outputs: IndexMap::from([
            ("one".to_string(), RecordOutput::File(a.clone())),
            (
                "keyed".to_string(),
                RecordOutput::Collection(vec![
                    ("bo".to_string(), RecordOutput::File(b.clone())),
                    (
                        "nested".to_string(),
                        RecordOutput::List(vec![RecordOutput::None, RecordOutput::File(a.clone())]),
                    ),
                ]),
            ),
            ("many".to_string(), RecordOutput::List(vec![])),
            ("nothing".to_string(), RecordOutput::None),
        ]),
        facts: IndexMap::from([
            ("verdict".to_string(), json!("accept")),
            ("scores".to_string(), json!({"x": [0.5, 1]})),
        ]),
        read: ReadSet::from([
            ("image/0".to_string(), digest_of(3)),
            ("image/10".to_string(), digest_of(4)),
        ]),
        cost_usd: Some(Usd(123_456)),
    };
    assert_eq!(
        ResultRecord::from_value(&result.to_value()),
        Ok(result.clone())
    );
    let free = ResultRecord {
        cost_usd: None,
        ..result.clone()
    };
    assert_eq!(free.to_value()["cost_usd"], Value::Null);
    assert_eq!(ResultRecord::from_value(&free.to_value()), Ok(free));
    assert_eq!(
        result.outputs["keyed"].files(),
        vec![&b, &a],
        "files in order"
    );

    let call = replay_call(b"png");
    assert_eq!(CallRecord::from_value(&call.to_value()), Ok(call.clone()));
    let mut unreported = call.clone();
    unreported.cost_usd = None;
    unreported.data = json!({"n": 1, "seed": [1, 2]});
    assert_eq!(
        CallRecord::from_value(&unreported.to_value()),
        Ok(unreported)
    );
    // A call record's files never carry a key.
    let mut keyed = call.clone();
    keyed.files.get_mut("image").unwrap().key = Some("k".into());
    assert_eq!(keyed.to_value(), call.to_value());

    for record in [
        job(&digest_of(5), JobState::Submitting, None),
        job(
            &digest_of(5),
            JobState::Submitted,
            Some(json!({"request_id": "R1"})),
        ),
        job(&digest_of(5), JobState::Settled, None),
        job(
            &digest_of(5),
            JobState::Settled,
            Some(json!({"request_id": "R1"})),
        ),
    ] {
        assert_eq!(
            JobRecord::from_value(&record.to_value()),
            Ok(record.clone())
        );
    }
}

#[test]
fn readers_refuse_what_the_schemas_refuse() {
    let result = result_record(&store().1, &digest_of(1), ReadSet::new()).to_value();
    let call = replay_call(b"png").to_value();
    let job = job(&digest_of(5), JobState::Submitting, None).to_value();

    type Edit = fn(&mut Value);
    let result_edits: [(&str, Edit); 14] = [
        ("unknown member", |v| v["extra"] = json!(1)),
        ("missing member", |v| {
            v.as_object_mut().unwrap().remove("facts");
        }),
        ("kind", |v| v["kind"] = json!(CALL_KIND)),
        ("identity", |v| v["identity"] = json!("ABC")),
        ("type", |v| v["type"] = json!(1)),
        ("outputs", |v| v["outputs"] = json!([])),
        ("output shape", |v| {
            v["outputs"]["lines"] = json!({"file": {}})
        }),
        ("output member", |v| {
            v["outputs"]["lines"]["file"]["extra"] = json!(1)
        }),
        ("file size", |v| {
            v["outputs"]["lines"]["file"]["size"] = json!(-1)
        }),
        ("file key", |v| {
            v["outputs"]["lines"]["file"]["key"] = json!(1)
        }),
        ("facts", |v| v["facts"] = json!(null)),
        ("read name", |v| {
            v["read"] = json!({"text/01": ONE_TWO_DIGEST})
        }),
        ("read digest", |v| v["read"] = json!({"text/0": "c3f9"})),
        ("cost", |v| v["cost_usd"] = json!(0.0000001)),
    ];
    for (what, edit) in result_edits {
        let mut value = result.clone();
        edit(&mut value);
        assert!(ResultRecord::from_value(&value).is_err(), "{what}");
    }
    let mut negative = result.clone();
    negative["cost_usd"] = json!(-1);
    assert!(ResultRecord::from_value(&negative).is_err());

    let call_edits: [(&str, Edit); 12] = [
        ("unknown member", |v| v["extra"] = json!(1)),
        ("missing member", |v| {
            v.as_object_mut().unwrap().remove("data");
        }),
        ("kind", |v| v["kind"] = json!(RESULT_KIND)),
        ("key", |v| v["key"] = json!(null)),
        ("capability", |v| v["capability"] = json!("Image.generate")),
        ("route id", |v| v["route"]["id"] = json!("img-a")),
        ("route member", |v| v["route"]["extra"] = json!(1)),
        ("request", |v| v["request"] = json!("prompt")),
        ("empty take", |v| v["take"] = json!([])),
        ("zero take", |v| v["take"] = json!([0])),
        ("fractional take", |v| v["take"] = json!([1.5])),
        ("keyed file", |v| v["files"]["image"]["key"] = json!("k")),
    ];
    for (what, edit) in call_edits {
        let mut value = call.clone();
        edit(&mut value);
        assert!(CallRecord::from_value(&value).is_err(), "{what}");
    }

    let job_edits: [(&str, Edit); 6] = [
        ("unknown member", |v| v["extra"] = json!(1)),
        ("missing handle", |v| {
            v.as_object_mut().unwrap().remove("handle");
        }),
        ("kind", |v| v["kind"] = json!(CALL_KIND)),
        ("state", |v| v["state"] = json!("done")),
        ("handle while submitting", |v| {
            v["handle"] = json!({"id": 1})
        }),
        ("take", |v| v["take"] = json!(1)),
    ];
    for (what, edit) in job_edits {
        let mut value = job.clone();
        edit(&mut value);
        assert!(JobRecord::from_value(&value).is_err(), "{what}");
    }
    assert_eq!(job["kind"], json!(JOB_KIND));
}

#[test]
fn output_values_encode_and_decode() {
    let (_dir, store) = store();
    let mut keyed = file(ONE_TWO, "text/plain", "count/lines[ada]");
    keyed.key = Some("ada".into());
    let mut blank = file(ONE_TWO, "text/plain", "count/lines");
    blank.key = Some(String::new());
    let collection = Val::Collection(Box::new(Collection {
        items: vec![
            ("ada".into(), Val::File(Box::new(keyed.clone()))),
            ("bo".into(), Val::List(vec![Val::Missing, Val::Null])),
        ],
        verdicts: IndexMap::new(),
    }));
    let output = RecordOutput::from_val(&collection).unwrap();
    assert_eq!(
        output.to_value(),
        json!({"collection": [
            ["ada", {"file": {"digest": ONE_TWO_DIGEST, "kind": "text/plain",
                              "name": "count/lines[ada]", "size": 8, "key": "ada"}}],
            ["bo", {"list": [{"none": true}, {"none": true}]}],
        ]})
    );
    assert_eq!(
        RecordOutput::from_val(&Val::File(Box::new(blank))).unwrap(),
        RecordOutput::File(entry(ONE_TWO, "text/plain", "count/lines"))
    );
    for (value, word) in [
        (Val::Str("x".into()), "text"),
        (Val::Number(1.0), "a number"),
        (Val::Object(IndexMap::new()), "an object"),
        (Val::Failed("a#1".into()), "a failed result"),
    ] {
        assert_eq!(
            RecordOutput::from_val(&value),
            Err(format!(
                "a step output is a file, a list or a collection, not {word}"
            ))
        );
    }
    assert!(RecordOutput::from_val(&Val::List(vec![Val::Bool(true)])).is_err());

    store.put_bytes(ONE_TWO).unwrap();
    let back = output.to_val(&store).unwrap();
    let Val::Collection(back) = back else {
        panic!("a collection");
    };
    assert!(back.verdicts.is_empty());
    let Val::File(first) = &back.items[0].1 else {
        panic!("a file");
    };
    assert_eq!(**first, keyed);
    assert_eq!(first.content, Some(FileContent::Text("one\ntwo\n".into())));
    assert_eq!(
        first.location,
        Some(store.file_path(ONE_TWO_DIGEST).unwrap())
    );
    assert_eq!(back.items[1].1, Val::List(vec![Val::Null, Val::Null]));
    assert_eq!(RecordOutput::None.to_val(&store).unwrap(), Val::Null);
}

// -------------------------------------------------------------------- trust

#[test]
fn a_result_record_is_trusted_only_when_every_check_passes() {
    let (_dir, store) = store();
    let identity = digest_of(0x21);
    let read = ReadSet::from([("text/0".to_string(), ONE_TWO_DIGEST.to_string())]);
    assert!(store.load_result(&identity, &read).is_none(), "absent");
    let record = result_record(&store, &identity, read.clone());
    store.save_result(&record).unwrap();
    assert_eq!(store.load_result(&identity, &read), Some(record.clone()));

    // The read set must be equal, exactly.
    let mut other = read.clone();
    other.insert("text/0".into(), digest_of(9));
    assert!(store.load_result(&identity, &other).is_none());
    let mut more = read.clone();
    more.insert("text/1".into(), ONE_TWO_DIGEST.into());
    assert!(store.load_result(&identity, &more).is_none());
    assert!(store.load_result(&identity, &ReadSet::new()).is_none());

    let path = store
        .root()
        .join("results")
        .join(&identity[..2])
        .join(format!("{identity}.json"));
    let good = fs::read(&path).unwrap();

    // Under another identity's name.
    let elsewhere = digest_of(0x22);
    let elsewhere_path = store
        .root()
        .join("results")
        .join("22")
        .join(format!("{elsewhere}.json"));
    fs::create_dir_all(elsewhere_path.parent().unwrap()).unwrap();
    fs::write(&elsewhere_path, &good).unwrap();
    assert!(store.load_result(&elsewhere, &read).is_none());

    let rewrite = |bytes: &[u8]| {
        make_writable(&path);
        fs::write(&path, bytes).unwrap();
    };
    let edited = |edit: fn(&mut Value)| {
        let mut value: Value = serde_json::from_slice(&good).unwrap();
        edit(&mut value);
        canon(&value).into_bytes()
    };
    for (what, bytes) in [
        ("not JSON", b"{\"kind\":".to_vec()),
        ("not UTF-8", b"\xff\xfe".to_vec()),
        ("duplicate keys", {
            let text = String::from_utf8(good.clone()).unwrap();
            text.replacen("{", "{\"type\":\"x\",", 1).into_bytes()
        }),
        ("wrong kind", edited(|v| v["kind"] = json!(CALL_KIND))),
        ("unknown member", edited(|v| v["extra"] = json!(true))),
        (
            "missing file",
            edited(|v| v["outputs"]["lines"]["file"]["digest"] = json!(digest_of(7))),
        ),
        (
            "other size",
            edited(|v| v["outputs"]["lines"]["file"]["size"] = json!(9)),
        ),
    ] {
        rewrite(&bytes);
        assert!(store.load_result(&identity, &read).is_none(), "{what}");
    }
    rewrite(&good);
    assert!(store.load_result(&identity, &read).is_some());

    // A missing output file makes it absent.
    let file_path = store.file_path(ONE_TWO_DIGEST).unwrap();
    fs::remove_file(&file_path).unwrap();
    assert!(store.load_result(&identity, &read).is_none());
}

#[test]
fn a_record_is_published_only_after_its_files() {
    let (_dir, store) = store();
    let mut record = result_record(&store, &digest_of(0x31), ReadSet::new());
    record.outputs.insert(
        "later".into(),
        RecordOutput::File(entry(b"not stored", "file", "later")),
    );
    let error = store.save_result(&record).unwrap_err();
    assert!(matches!(error, StoreError::Io { .. }), "{error}");
    assert!(error.to_string().contains("not in the store"), "{error}");
    assert!(!store.root().join("results").exists());

    let call = replay_call(b"not stored");
    let error = store.save_call(&call).unwrap_err();
    assert!(matches!(error, StoreError::Io { .. }), "{error}");
    assert!(!store.root().join("calls").exists());
}

#[test]
fn a_call_record_is_trusted_only_when_every_check_passes() {
    let (_dir, store) = store();
    let image = b"\x89PNG stand-in".to_vec();
    let record = {
        let mut record = replay_call(&image);
        record.key = record.computed_key();
        record
    };
    let key = record.key.clone();
    assert!(store.load_call(&key).is_none(), "absent");
    store.put_bytes(&image).unwrap();
    store.save_call(&record).unwrap();
    assert_eq!(store.load_call(&key), Some(record.clone()));

    // A key that does not recompute is refused on the way in.
    let mut wrong = record.clone();
    wrong.take = vec![2];
    let error = store.save_call(&wrong).unwrap_err();
    assert!(error.to_string().contains("call key"), "{error}");

    let path = store
        .root()
        .join("calls")
        .join(&key[..2])
        .join(format!("{key}.json"));
    let good = fs::read(&path).unwrap();

    // Under another key's name.
    let elsewhere = digest_of(0x44);
    let elsewhere_path = store
        .root()
        .join("calls")
        .join("44")
        .join(format!("{elsewhere}.json"));
    fs::create_dir_all(elsewhere_path.parent().unwrap()).unwrap();
    fs::write(&elsewhere_path, &good).unwrap();
    assert!(store.load_call(&elsewhere).is_none());

    let rewrite = |bytes: &[u8]| {
        make_writable(&path);
        fs::write(&path, bytes).unwrap();
    };
    let edited = |edit: fn(&mut Value)| {
        let mut value: Value = serde_json::from_slice(&good).unwrap();
        edit(&mut value);
        canon(&value).into_bytes()
    };
    for (what, bytes) in [
        ("not JSON", b"[".to_vec()),
        ("wrong kind", edited(|v| v["kind"] = json!(JOB_KIND))),
        ("unknown member", edited(|v| v["extra"] = json!(1))),
        // The key no longer recomputes (store.md §3 MUST): absent.
        (
            "request edited",
            edited(|v| v["request"]["prompt"] = json!("a candle")),
        ),
        ("take edited", edited(|v| v["take"] = json!([2]))),
        (
            "missing file",
            edited(|v| v["files"]["image"]["digest"] = json!(digest_of(8))),
        ),
        (
            "other size",
            edited(|v| v["files"]["image"]["size"] = json!(1)),
        ),
    ] {
        rewrite(&bytes);
        assert!(store.load_call(&key).is_none(), "{what}");
    }
    rewrite(&good);
    assert!(store.load_call(&key).is_some());
    fs::remove_file(store.file_path(&file_digest(&image)).unwrap()).unwrap();
    assert!(store.load_call(&key).is_none(), "file gone");
}

// --------------------------------------------------------------------- jobs

#[test]
fn job_records_move_through_their_states() {
    let (_dir, store) = store();
    let key = digest_of(0x51);
    assert_eq!(store.load_job(&key), Ok(None));
    assert_eq!(store.remove_job(&key), Ok(()));
    assert_eq!(store.jobs(), Ok(vec![]));

    let submitting = job(&key, JobState::Submitting, None);
    store.save_job(&submitting).unwrap();
    assert_eq!(store.load_job(&key), Ok(Some(submitting)));
    let path = store.root().join("jobs").join(format!("{key}.json"));
    assert!(!fs::metadata(&path).unwrap().permissions().readonly());

    let submitted = job(&key, JobState::Submitted, Some(json!({"request_id": "R1"})));
    store.save_job(&submitted).unwrap();
    assert_eq!(store.load_job(&key), Ok(Some(submitted.clone())));
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        canon(&submitted.to_value())
    );
    assert_eq!(
        names(path.parent().unwrap()),
        vec![format!("{key}.json")],
        "no temporary file"
    );

    // A record that breaks the state rules is refused on the way in.
    let error = store
        .save_job(&job(&key, JobState::Submitted, None))
        .unwrap_err();
    assert!(matches!(error, StoreError::Io { .. }), "{error}");
    assert_eq!(store.load_job(&key), Ok(Some(submitted)));

    store.remove_job(&key).unwrap();
    assert_eq!(store.load_job(&key), Ok(None));
    assert_eq!(store.remove_job(&key), Ok(()));
}

#[test]
fn every_job_is_listed_in_key_order() {
    let (_dir, store) = store();
    let keys = [digest_of(0x99), digest_of(0x10), digest_of(0x5a)];
    for key in &keys {
        store.save_job(&job(key, JobState::Settled, None)).unwrap();
    }
    let folder = store.root().join("jobs");
    // Not records: skipped.
    fs::write(folder.join(".0123456789abcdef.part"), b"half").unwrap();
    fs::write(folder.join("README"), b"notes").unwrap();
    fs::write(folder.join(format!("{}.json.bak", digest_of(1))), b"x").unwrap();
    fs::write(folder.join("ABC.json"), b"x").unwrap();
    let listed: Vec<String> = store.jobs().unwrap().into_iter().map(|j| j.key).collect();
    assert_eq!(
        listed,
        vec![digest_of(0x10), digest_of(0x5a), digest_of(0x99)]
    );
}

#[test]
fn an_unreadable_job_record_is_an_error_naming_it() {
    let (dir, store) = store();
    let good = digest_of(0x10);
    store
        .save_job(&job(&good, JobState::Settled, None))
        .unwrap();
    let folder = store.root().join("jobs");
    let root = dir.path().to_string_lossy().into_owned();
    let other = digest_of(0x20);
    for (what, bytes) in [
        ("not JSON", b"{".to_vec()),
        ("not UTF-8", b"\xff".to_vec()),
        ("not a record", b"{}".to_vec()),
        (
            "bad state",
            canon(&{
                let mut v = job(&other, JobState::Settled, None).to_value();
                v["state"] = json!("lost");
                v
            })
            .into_bytes(),
        ),
        (
            "another key",
            canon(&job(&good, JobState::Settled, None).to_value()).into_bytes(),
        ),
    ] {
        fs::write(folder.join(format!("{other}.json")), &bytes).unwrap();
        let error = store.load_job(&other).unwrap_err();
        assert!(
            matches!(&error, StoreError::UnreadableJob { key, .. } if *key == other),
            "{what}: {error}"
        );
        let text = error.to_string();
        assert!(
            text.starts_with(&format!("the job record jobs/{other}.json is unreadable: ")),
            "{what}: {text}"
        );
        assert!(!text.contains(&root), "{what}: {text}");
        // Listing stops at it too.
        assert_eq!(store.jobs(), Err(error), "{what}");
    }
    // A folder where a record should be is unreadable too, not absent.
    fs::remove_file(folder.join(format!("{other}.json"))).unwrap();
    fs::create_dir(folder.join(format!("{other}.json"))).unwrap();
    assert!(matches!(
        store.load_job(&other),
        Err(StoreError::UnreadableJob { .. })
    ));
}

// ---------------------------------------------------------------- read sets

#[test]
fn a_read_set_numbers_each_values_files_in_order() {
    let f = |n: u8| {
        let bytes = [n];
        Val::File(Box::new(file(&bytes, "file", &format!("f{n}"))))
    };
    let d = |n: u8| file_digest(&[n]);
    let with: IndexMap<String, Val> = IndexMap::from([
        ("prompt".to_string(), Val::Str("a lantern".into())),
        ("image".to_string(), f(1)),
        (
            "refs".to_string(),
            Val::List(vec![
                f(2),
                Val::Collection(Box::new(Collection {
                    items: vec![
                        ("z".into(), f(3)),
                        ("a".into(), Val::List(vec![f(4), Val::Missing, f(1)])),
                    ],
                    verdicts: IndexMap::new(),
                })),
                Val::Object(IndexMap::from([
                    ("y".to_string(), f(5)),
                    (
                        "b".to_string(),
                        Val::Object(IndexMap::from([("c".into(), f(6))])),
                    ),
                ])),
                Val::Null,
                f(7),
                f(8),
                f(9),
                f(10),
                f(11),
                f(12),
            ]),
        ),
        ("gone".to_string(), Val::Failed("x#1".into())),
        ("waiting".to_string(), Val::Missing),
    ]);
    let expected: ReadSet = [
        ("image/0", d(1)),
        ("refs/0", d(2)),
        ("refs/1", d(3)),
        ("refs/2", d(4)),
        ("refs/3", d(1)),
        ("refs/4", d(5)),
        ("refs/5", d(6)),
        ("refs/6", d(7)),
        ("refs/7", d(8)),
        ("refs/8", d(9)),
        ("refs/9", d(10)),
        ("refs/10", d(11)),
        ("refs/11", d(12)),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    let set = read_set(&with);
    assert_eq!(set, expected);
    // Sorted by key as text: refs/10 comes before refs/2.
    let keys: Vec<&String> = set.keys().collect();
    assert_eq!(keys[1], "refs/0");
    assert_eq!(keys[2], "refs/1");
    assert_eq!(keys[3], "refs/10");
    let names: Vec<&str> = files_in(&with["refs"])
        .into_iter()
        .map(|f| f.name.as_str())
        .collect();
    assert_eq!(
        names,
        [
            "f2", "f3", "f4", "f1", "f5", "f6", "f7", "f8", "f9", "f10", "f11", "f12"
        ]
    );
    assert!(files_in(&Val::Str("x".into())).is_empty());
    assert!(read_set(&IndexMap::new()).is_empty());
}

#[test]
fn planning_sees_a_trusted_result_through_the_read_set() {
    let (_dir, store) = store();
    let identity = digest_of(0x61);
    let with = IndexMap::from([(
        "text".to_string(),
        Val::File(Box::new(file(ONE_TWO, "text/plain", "lines.txt"))),
    )]);
    let asking = instance(Some(&identity), with.clone());
    assert!(!store.has_result(&asking));
    store
        .save_result(&result_record(&store, &identity, read_set(&with)))
        .unwrap();
    assert!(store.has_result(&asking));
    // Other bytes read: not cached.
    let other = IndexMap::from([(
        "text".to_string(),
        Val::File(Box::new(file(b"three\n", "text/plain", "lines.txt"))),
    )]);
    assert!(!store.has_result(&instance(Some(&identity), other)));
    // No identity yet: not cached.
    assert!(!store.has_result(&instance(None, with)));
}

// ------------------------------------------------------------------ vectors

#[test]
fn call_keys_match_the_spec_examples() {
    assert_eq!(
        call_key(
            "image.generate",
            FINGERPRINT,
            &json!({"background": "auto", "prompt": "A picture of 2 lines"}),
            &[1]
        ),
        "9f66a9f2d23c7cf21331a3a45778bf9a660451a79cd4df3aae5f576e0b807d38"
    );
    assert_eq!(
        call_key(
            "image.generate",
            FINGERPRINT,
            &json!({"prompt": "a lantern", "background": "auto"}),
            &[1]
        ),
        REPLAY_KEY
    );
    assert_eq!(
        call_key(
            "speech.generate",
            "3f38f8fe328b6466eb3ec1605239fab8e364c55ee340bd7e9ee49b2644993675",
            &json!({"text": "Hello.\r\n", "voice": "narrator-a"}),
            &[1]
        ),
        "a789cf0e3723acd1caa7e89f4049e93fb445916b48c52676fdbdc26981c638d8"
    );
}

#[test]
fn the_cache_replay_store_answers_its_call() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("cache");
    copy_tree(&replay_cache(), &root);
    let store = Store::open(&root);
    let record = store.load_call(REPLAY_KEY).expect("a trusted call record");
    assert_eq!(record.computed_key(), REPLAY_KEY);
    assert_eq!(record.capability, "image.generate");
    assert_eq!(record.route.id, "img-a@acme");
    assert_eq!(record.take, vec![1]);
    assert_eq!(record.cost_usd, Some(Usd(20_000)));
    assert_eq!(record.data, Value::Null);
    let image = store.file_value(&record.files["image"]).unwrap();
    assert_eq!(image.digest, REPLAY_IMAGE);
    assert_eq!(image.size, 70);
    assert_eq!(image.kind, "image/png");
    assert_eq!(image.content, None);
    assert_eq!(image.location, Some(store.file_path(REPLAY_IMAGE).unwrap()));
    assert_eq!(store.verify(REPLAY_IMAGE), Ok(true));
    // Without its file, the record is absent.
    let copy = store.file_path(REPLAY_IMAGE).unwrap();
    make_writable(&copy);
    fs::remove_file(&copy).unwrap();
    assert!(store.load_call(REPLAY_KEY).is_none());
}

// ------------------------------------------------------------------ what a killed run leaves

/// Makes a file or folder look `hours` old.
fn age(path: &Path, hours: u64) {
    let when = std::time::SystemTime::now() - std::time::Duration::from_secs(hours * 3600);
    let file = fs::File::open(path).unwrap();
    file.set_modified(when).unwrap();
}

#[test]
fn stale_temporary_files_are_swept_and_fresh_ones_kept() {
    let (_dir, store) = store();
    let stored = store.put_bytes(ONE_TWO).unwrap();
    let fan = store.file_path(&stored.digest).unwrap();
    let fan = fan.parent().unwrap();
    let old = fan.join(".0123456789abcdef.part");
    let fresh = fan.join(".fedcba9876543210.part");
    let record_temp = store.root().join("results/ab/.00000000000000aa.part");
    let job_temp = store.root().join("jobs/.00000000000000bb.part");
    fs::create_dir_all(record_temp.parent().unwrap()).unwrap();
    fs::create_dir_all(job_temp.parent().unwrap()).unwrap();
    for path in [&old, &fresh, &record_temp, &job_temp] {
        fs::write(path, b"partial").unwrap();
    }
    for path in [&old, &record_temp, &job_temp] {
        age(path, 2);
    }
    store.sweep();
    assert!(!old.exists() && !record_temp.exists() && !job_temp.exists());
    assert!(
        fresh.exists(),
        "a temporary file being written is left alone"
    );
    assert!(store.has(&stored.digest, stored.size), "stored files stay");
}

#[test]
fn work_dirs_of_invocations_that_are_gone_are_swept() {
    let (_dir, store) = store();
    let work = store.work_root();
    // An invocation of this process that still runs: its claim holds.
    store.claim_work("1111111111111111").unwrap();
    store.claim_work("1111111111111111").unwrap();
    fs::create_dir_all(work.join("1111111111111111-1/out")).unwrap();
    // A killed invocation: its lock file is there, and no one holds it.
    fs::write(work.join("2222222222222222.lock"), b"").unwrap();
    fs::create_dir_all(work.join("2222222222222222-1/out")).unwrap();
    fs::write(work.join("2222222222222222-1/out/big.bin"), b"x").unwrap();
    fs::create_dir_all(work.join("2222222222222222-2")).unwrap();
    // Dirs no claim covers: swept once an hour old.
    fs::create_dir_all(work.join("plan-1")).unwrap();
    age(&work.join("plan-1"), 2);
    fs::create_dir_all(work.join("plan-2")).unwrap();
    store.sweep();
    let mut left = names(&work);
    left.sort();
    assert_eq!(
        left,
        ["1111111111111111-1", "1111111111111111.lock", "plan-2"]
    );
    store.release_work("1111111111111111");
    assert!(!work.join("1111111111111111.lock").exists());
    // A dir its run left behind with no claim goes once it is an hour old.
    age(&work.join("1111111111111111-1"), 2);
    store.sweep();
    let mut left = names(&work);
    left.sort();
    assert_eq!(left, ["plan-2"]);
}

#[test]
fn a_source_that_cannot_be_read_is_not_the_stores_fault() {
    let (dir, store) = store();
    let gone = dir.path().join("gone.txt");
    match store.put_file(&gone, "out/gone.txt") {
        Err(StoreError::Source(sentence)) => {
            assert_eq!(sentence, "cannot read out/gone.txt: no such file")
        }
        other => panic!("{other:?}"),
    }
}
