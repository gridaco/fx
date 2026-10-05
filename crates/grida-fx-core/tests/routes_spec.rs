//! Money, routes, node specs, the built-in catalog, file kinds and file facts through the public
//! API: identity.md §4, §6, §7, §12 and §14, protocol.md §4, and the conformance `facts` fixture.

use grida_fx_core::ErrorKind;
use grida_fx_core::builtins::{builtin, builtins};
use grida_fx_core::docs::project::{Project, ProjectDoc};
use grida_fx_core::facts::{ImageFacts, file_facts, image_facts};
use grida_fx_core::kinds::{effective_kind, kind_of, suffix_of_kind};
use grida_fx_core::money::{Units, Usd};
use grida_fx_core::routes::{PriceUnit, Route, RoutePrice, RouteTable, load_catalog};
use grida_fx_core::spec::{BodyKind, NodeSpec, Port, Shape};
use grida_fx_core::val::{Pending, Val};
use grida_fx_protocol::TypeSpec;
use indexmap::IndexMap;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::PathBuf;

fn repository(path: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(path)
}

fn route(capability: &str, model: &str, provider: &str, contract: Value) -> Route {
    Route {
        capability: capability.into(),
        model: model.into(),
        provider: provider.into(),
        price: RoutePrice {
            low: Usd::ZERO,
            high: Usd::ZERO,
            unit: PriceUnit::Call,
            max_units: None,
            by: None,
            tiers: IndexMap::new(),
        },
        features: BTreeSet::new(),
        concurrency: None,
        requests_per_minute: None,
        contract,
    }
}

fn priced(price: RoutePrice) -> Route {
    Route {
        price,
        ..route("c", "m", "p", json!({}))
    }
}

fn with(values: &[(&str, Val)]) -> IndexMap<String, Val> {
    values
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

fn micros(pair: (Usd, Usd)) -> (i64, i64) {
    (pair.0.0, pair.1.0)
}

#[test]
fn route_fingerprints_of_the_worked_examples() {
    let generate = route("image.generate", "img-a", "acme", json!({}));
    assert_eq!(generate.id(), "img-a@acme");
    assert_eq!(
        generate.fingerprint(),
        "4d41b81c56215efdd18574eab8e2b704a8ecc86af08d569f55cd29973c4e7ed4"
    );
    let edit = route(
        "image.edit",
        "img-a",
        "acme",
        json!({"mask": true, "sizes": ["1024x1024", "1536x1024"]}),
    );
    assert_eq!(
        edit.fingerprint(),
        "5d727add4c417db9508eb6c7f81ff19f7d90005adf080c0702acb9077a330d6b"
    );
    let speech = route("speech.generate", "voice-a", "acme", json!({}));
    assert_eq!(
        speech.fingerprint(),
        "3f38f8fe328b6466eb3ec1605239fab8e364c55ee340bd7e9ee49b2644993675"
    );
    // The contract's arrays keep their order.
    let reordered = route(
        "image.edit",
        "img-a",
        "acme",
        json!({"sizes": ["1536x1024", "1024x1024"], "mask": true}),
    );
    assert_ne!(reordered.fingerprint(), edit.fingerprint());
    assert_eq!(
        route("x", "Vendor/IMG-A", "p", json!({})).underlying_model(),
        "img-a"
    );
}

/// The conformance `tiered-price` route: per second by resolution, at most 10 seconds.
fn tiered() -> Route {
    priced(RoutePrice {
        low: Usd(30_000),
        high: Usd(375_000),
        unit: PriceUnit::Second,
        max_units: Some(Units::whole(10)),
        by: Some("resolution".into()),
        tiers: IndexMap::from([
            ("360p".to_string(), (Usd(30_000), Usd(37_500))),
            ("4k".to_string(), (Usd(300_000), Usd(375_000))),
        ]),
    })
}

#[test]
fn the_tiered_price_case() {
    let route = tiered();
    let small = route.cost(&with(&[
        ("duration", Val::Number(3.0)),
        ("resolution", Val::Str("360p".into())),
    ]));
    let large = route.cost(&with(&[
        ("duration", Val::Number(8.0)),
        ("resolution", Val::Str("4k".into())),
    ]));
    let unknown = route.cost(&with(&[("prompt", Val::Str("a lantern sways".into()))]));
    assert_eq!(micros(small), (90_000, 112_500));
    assert_eq!(micros(large), (2_400_000, 3_000_000));
    assert_eq!(micros(unknown), (0, 3_750_000));
    let low: Usd = [small.0, large.0, unknown.0].into_iter().sum();
    let high: Usd = [small.1, large.1, unknown.1].into_iter().sum();
    assert_eq!((low, high), (Usd(2_490_000), Usd(6_862_500)));
    assert_eq!(low.to_value(), json!(2.49));
    assert_eq!(high.to_value(), json!(6.8625));
    assert_eq!(
        format!("{} – {}", low.dollars_2(), high.dollars_2()),
        "$2.49 – $6.86"
    );
}

#[test]
fn length_priced_calls() {
    let speech = priced(RoutePrice {
        low: Usd(100_000),
        high: Usd(300_000),
        unit: PriceUnit::KChars,
        max_units: Some(Units::whole(5000)),
        by: None,
        tiers: IndexMap::new(),
    });
    // 11 characters: Unicode scalar values, not bytes.
    let text = Val::Str("h\u{e9}llo w\u{f6}rld".into());
    assert_eq!(
        micros(speech.cost(&with(&[("text", text)]))),
        (1_100, 3_300)
    );
    let pending = Val::Pending(Box::new(Pending {
        refs: BTreeSet::from(["write#1".to_string()]),
        token: "token".into(),
    }));
    assert_eq!(
        micros(speech.cost(&with(&[
            ("text", pending.clone()),
            ("max_chars", Val::Number(1234.0)),
        ]))),
        (123_400, 370_200)
    );
    assert_eq!(
        micros(speech.cost(&with(&[("text", pending)]))),
        (0, 1_500_000)
    );
    assert_eq!(
        micros(speech.cost(&with(&[("max_chars", Val::Bool(true))]))),
        (0, 1_500_000)
    );
}

#[test]
fn call_counts_multiply_whole_prices() {
    // The predecessor's 7 × (0.0123456 – 0.0333333) has 7-place prices, which FX refuses when a
    // table is read; the same count over 6-place prices is exact.
    let per_call = priced(RoutePrice {
        low: Usd(12_346),
        high: Usd(33_333),
        unit: PriceUnit::Call,
        max_units: None,
        by: None,
        tiers: IndexMap::new(),
    })
    .cost(&IndexMap::new());
    assert_eq!(
        (per_call.0.times(7), per_call.1.times(7)),
        (Usd(86_422), Usd(233_331))
    );
    let spec = NodeSpec::from_type_spec(
        &wire(json!({
            "params": {"n": {"type": "integer", "minimum": 1, "maximum": 4}},
            "calls": {"image.generate": "n"},
        })),
        None,
    )
    .unwrap();
    // A step's own count is not capped at the maximum.
    let calls = spec.capability_calls(&with(&[("n", Val::Number(7.0))]));
    assert_eq!(calls["image.generate"], 7);
    assert_eq!(spec.capability_calls(&IndexMap::new())["image.generate"], 4);
}

#[test]
fn money_rules() {
    assert_eq!(Usd::parse("1.5"), Ok(Usd(1_500_000)));
    assert_eq!(Usd::parse("0.000001"), Ok(Usd(1)));
    assert_eq!(Usd::parse("2e-6"), Ok(Usd(2)));
    for refused in ["nan", "inf", "-1", "0.0000001", "1e-7", "one", ""] {
        assert!(Usd::parse(refused).is_err(), "{refused}");
    }
    assert_eq!(Usd::from_value(&json!(0.000001)), Ok(Usd(1)));
    assert_eq!(
        Usd::from_value(&json!(0.0000001)).unwrap_err(),
        "1e-7 has more than 6 decimal places"
    );
    assert!(Usd::from_value(&json!(-0.5)).is_err());
    assert_eq!(Usd(0).to_value(), json!(0));
    assert_eq!(Usd(40_000).to_value(), json!(0.04));
    assert_eq!(Usd(40_000).to_string(), "0.04");
    assert_eq!(Usd(3_750_000).dollars_2(), "$3.75");
    assert_eq!(Usd(125_000).dollars_2(), "$0.12");
    assert_eq!(Usd(1_005_000).dollars_2(), "$1.00");
    assert_eq!(
        Usd(1).times_units(Units::from_f64(2.5), 1),
        Usd(2),
        "half to even"
    );
}

#[test]
fn route_tables_from_documents() {
    let document = json!({"fx": "routes/v1", "routes": [
        {"capability": "image.generate", "route": "img-a@acme", "price": {"low_usd": 0.01, "high_usd": 0.04}, "features": ["alpha"]},
        {"capability": "image.edit", "route": "img-a@acme", "price": {"usd": 0.05}},
    ]});
    let mut catalog = RouteTable::from_document(&document, "routes.yaml").unwrap();
    assert_eq!(catalog.entries.len(), 2);
    let later = json!({"fx": "routes/v1", "routes": [
        {"capability": "image.generate", "route": "img-a@acme", "price": {"usd": 0.02}, "contract": {"quality": "high"}},
    ]});
    catalog.overlay(RouteTable::from_document(&later, "more.yaml").unwrap());
    let replaced = catalog.resolve("image.generate", "img-a@acme").unwrap();
    assert_eq!(replaced.price.high, Usd(20_000));
    assert!(replaced.features.is_empty());
    assert_eq!(replaced.contract, json!({"quality": "high"}));

    let twice = json!({"fx": "routes/v1", "routes": [
        {"capability": "image.generate", "route": "img-a@acme", "price": {"usd": 0.02}},
        {"capability": "image.generate", "route": "img-a@acme", "price": {"usd": 0.03}},
    ]});
    let error = RouteTable::from_document(&twice, "routes.yaml").unwrap_err();
    assert_eq!(error.kind, ErrorKind::Route);
    assert_eq!(
        error.message,
        "routes.yaml: routes.1: img-a@acme is declared twice for image.generate"
    );
    let places = json!({"fx": "routes/v1", "routes": [
        {"capability": "image.generate", "route": "img-a@acme", "price": {"low_usd": 0.0123456, "high_usd": 0.0333333}},
    ]});
    let error = RouteTable::from_document(&places, "routes.yaml").unwrap_err();
    assert_eq!(
        error.message,
        "routes.yaml: routes.0.price.low_usd: 0.0123456 has more than 6 decimal places"
    );
    let range = json!({"fx": "routes/v1", "routes": [
        {"capability": "image.generate", "route": "img-a@acme", "price": {"low_usd": 0.05, "high_usd": 0.04}},
    ]});
    assert_eq!(
        RouteTable::from_document(&range, "routes.yaml")
            .unwrap_err()
            .message,
        "routes.yaml: routes.0.price: a price is a non-negative range, low to high"
    );
}

#[test]
fn catalogs_combine_in_order() {
    let folder = tempfile::tempdir().unwrap();
    let root = folder.path().to_path_buf();
    std::fs::create_dir(root.join("tables")).unwrap();
    std::fs::write(
        root.join("tables/project.yaml"),
        "fx: routes/v1\nroutes:\n  - { capability: image.generate, route: img-a@acme, price: { usd: 0.04 } }\n  - { capability: image.edit, route: img-a@acme, price: { usd: 0.05 } }\n",
    )
    .unwrap();
    std::fs::write(
        root.join("extra.yaml"),
        "fx: routes/v1\nroutes:\n  - { capability: image.generate, route: img-a@acme, price: { usd: 0.01 } }\n",
    )
    .unwrap();
    let project = Project {
        root: root.clone(),
        document: ProjectDoc {
            route_tables: vec!["tables/project.yaml".into()],
            ..ProjectDoc::default()
        },
        has_file: true,
    };
    let catalog =
        load_catalog(&project, &[(root.join("extra.yaml"), "extra.yaml".into())]).unwrap();
    assert_eq!(catalog.entries.len(), 2);
    assert_eq!(
        catalog
            .resolve("image.generate", "img-a@acme")
            .unwrap()
            .price
            .high,
        Usd(10_000)
    );
    let only_project = load_catalog(&project, &[]).unwrap();
    assert_eq!(
        only_project
            .resolve("image.generate", "img-a@acme")
            .unwrap()
            .price
            .high,
        Usd(40_000)
    );
    let missing =
        load_catalog(&project, &[(root.join("none.yaml"), "none.yaml".into())]).unwrap_err();
    assert_eq!(missing.kind, ErrorKind::Io);
    assert!(
        missing.message.starts_with("none.yaml"),
        "{}",
        missing.message
    );
    assert!(!missing.message.contains(root.to_str().unwrap()));
}

#[test]
fn the_built_in_catalog() {
    let all = builtins();
    assert_eq!(all.len(), 37);
    let count = |word: &str| all.iter().filter(|b| b.spec.kind_word() == word).count();
    assert_eq!((count("paid"), count("judge"), count("free")), (11, 5, 21));
    assert_eq!(all.iter().filter(|b| b.spec.paid()).count(), 13);
    for b in all {
        assert_eq!(b.uses, format!("fx/{}@{}", b.name, b.major));
        assert_eq!(b.identity(), format!("fx/{}@1.1", b.name));
        assert_eq!(b.spec.capability.is_some(), b.body == BodyKind::Capability);
        if let Some(capability) = &b.spec.capability {
            assert_eq!(capability, &b.name);
        }
        // Every spec survives the wire form and validates again.
        let again = NodeSpec::from_type_spec(&b.spec.to_type_spec(), b.spec.capability.clone());
        assert_eq!(again.as_ref(), Ok(&b.spec), "{}", b.uses);
    }
    let generate = builtin("image.generate", 1).unwrap();
    assert_eq!(generate.identity(), "fx/image.generate@1.1");
    assert!(generate.spec.is_template("prompt"));
    assert!(generate.spec.is_optional_param("size"));
    assert_eq!(generate.spec.default_of("background"), Some(&json!("auto")));
    assert_eq!(generate.spec.default_of("vars"), Some(&json!({})));
    assert_eq!(generate.spec.inputs["references"].notation(), "image[]?");
    assert_eq!(generate.spec.outputs["image"].notation(), "image/png");
    assert_eq!(
        generate.spec.capability_calls(&IndexMap::new()),
        IndexMap::from([("image.generate".to_string(), 1)])
    );
    let select = builtin("select", 1).unwrap();
    assert_eq!(select.body, BodyKind::Engine);
    assert_eq!(select.spec.kind_word(), "free");
    let review = builtin("vision.review", 1).unwrap();
    assert_eq!(review.spec.kind_word(), "judge");
    assert!(review.spec.paid());
    let crop = builtin("image.crop", 1).unwrap();
    assert_eq!(crop.spec.default_of("padding"), Some(&json!(0)));
    assert!(builtin("image.generate", 2).is_none());
}

fn wire(value: Value) -> TypeSpec {
    let mut spec = json!({
        "name": "t", "inputs": {}, "params": {}, "outputs": {}, "judge": false, "calls": {},
        "resources": [], "tools": [], "view": null, "version": null, "retry": "service",
    });
    for (key, v) in value.as_object().unwrap() {
        spec[key] = v.clone();
    }
    serde_json::from_value(spec).unwrap()
}

#[test]
fn spec_validation_messages() {
    let refused = |value: Value| NodeSpec::from_type_spec(&wire(value), None).unwrap_err();
    let cases = [
        (
            json!({"name": "Draw"}),
            "node type name 'Draw' must be lower_snake words joined by .",
        ),
        (
            json!({"inputs": {"image": "image?[]"}}),
            "port 'image?[]' is not kind, kind[], kind{} with an optional ?",
        ),
        (
            json!({"outputs": {"image": "picture"}}),
            "port kind 'picture' is not one of ['annotations', 'audio', 'file', 'image', 'json', \
             'model', 'text', 'video']",
        ),
        (
            json!({"inputs": {"x": "image"}, "params": {"x": {"type": "string"}}}),
            "t: ['x'] declared as both input and param",
        ),
        (
            json!({"calls": {"image.generate": 0}}),
            "t: calls['image.generate'] must be at least 1",
        ),
        (
            json!({"params": {"n": {"type": "integer", "maximum": 3}}, "calls": {"c": "n"}}),
            "t: calls['c'] names 'n', which is not an integer setting with a minimum of at least 1",
        ),
        (
            json!({"params": {"n": {"type": "integer", "minimum": 1}}, "calls": {"c": "n"}}),
            "t: calls['c'] needs 'n' to set a maximum",
        ),
        (
            json!({"inputs": {"Image": "image"}}),
            "t: input name 'Image' must be a lower_snake word",
        ),
        (
            json!({"resources": ["../secret.md"]}),
            "t: resource '../secret.md' is not a POSIX path relative to the project root with no \
             empty, . or .. segment",
        ),
        (
            json!({"tools": ["Blender"]}),
            "t: tool 'Blender' is not a name of a-z, 0-9, _ and - with an optional version bound \
             such as >=4.2",
        ),
    ];
    for (value, message) in cases {
        assert_eq!(refused(value), message);
    }
    assert_eq!(
        NodeSpec::from_type_spec(&wire(json!({"calls": {"c": 1}})), Some("c".into())).unwrap_err(),
        "t: a capability type is its own one call"
    );
    let port = Port::parse("image{}?").unwrap();
    assert_eq!((port.shape, port.optional), (Shape::Keyed, true));
}

#[test]
fn file_kinds() {
    assert_eq!(kind_of("art.PNG"), "image/png");
    assert_eq!(kind_of("notes/brief.txt"), "text/plain");
    assert_eq!(kind_of("archive.tar"), "file");
    assert_eq!(suffix_of_kind("image/jpeg"), ".jpg");
    assert_eq!(suffix_of_kind("annotations"), ".json");
    assert_eq!(suffix_of_kind("file"), "");
    assert_eq!(effective_kind("a.txt", "image"), "text/plain");
    assert_eq!(effective_kind("a", "image"), "image");
}

#[test]
fn facts_of_the_conformance_picture() {
    let bytes = std::fs::read(repository("conformance/facts/in/art.png")).unwrap();
    assert_eq!(
        file_facts(&bytes, kind_of("art.png")).unwrap(),
        json!({"bytes": 82, "kind": "image/png", "width": 3, "height": 2, "has_alpha": true, "opaque": false})
    );
    let note = std::fs::read(repository("conformance/facts/in/note.txt")).unwrap();
    assert_eq!(
        file_facts(&note, "text/plain").unwrap(),
        json!({"bytes": 6, "kind": "text/plain"})
    );
}

#[test]
fn facts_of_synthesized_pictures() {
    // A 2×1 RGB PNG.
    let mut png = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png, 2, 1);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&[1, 2, 3, 4, 5, 6]).unwrap();
    }
    assert_eq!(
        image_facts(&png, "image/png").unwrap(),
        Some(ImageFacts {
            width: 2,
            height: 1,
            has_alpha: false,
            opaque: true
        })
    );
    // A 2×2 GIF whose transparent index 1 is used by one pixel.
    let mut gif = Vec::new();
    {
        let mut encoder = gif::Encoder::new(&mut gif, 2, 2, &[0, 0, 0, 255, 255, 255]).unwrap();
        let frame = gif::Frame {
            width: 2,
            height: 2,
            transparent: Some(1),
            buffer: std::borrow::Cow::Borrowed(&[0, 0, 1, 0]),
            ..gif::Frame::default()
        };
        encoder.write_frame(&frame).unwrap();
    }
    assert_eq!(
        image_facts(&gif, "image/gif").unwrap(),
        Some(ImageFacts {
            width: 2,
            height: 2,
            has_alpha: true,
            opaque: false
        })
    );
    // A JPEG's markers: SOI, then a baseline SOF0 of 640×480.
    let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x0B, 8];
    jpeg.extend(480u16.to_be_bytes());
    jpeg.extend(640u16.to_be_bytes());
    jpeg.extend([1, 1, 0x11, 0, 0xFF, 0xD9]);
    assert_eq!(
        file_facts(&jpeg, "image/jpeg").unwrap(),
        json!({"bytes": 17, "kind": "image/jpeg", "width": 640, "height": 480, "has_alpha": false, "opaque": true})
    );
    // 1×1 lossless WebPs (VP8L, alpha hint set) whose one pixel is coded with one-symbol prefix
    // codes: ARGB 0xFF102030, then the same with alpha 0x80. The facts module's unit tests build
    // these bytes bit by bit.
    let webp = |alpha: [u8; 2]| {
        let mut bytes = vec![
            82, 73, 70, 70, 26, 0, 0, 0, 87, 69, 66, 80, 86, 80, 56, 76, 13, 0, 0, 0, 47, 0, 0, 0,
            16, 40, 72, 33, 10,
        ];
        bytes.extend(alpha);
        bytes.extend([2, 0, 0]);
        bytes
    };
    assert_eq!(
        file_facts(&webp([211, 255]), "image/webp").unwrap(),
        json!({"bytes": 34, "kind": "image/webp", "width": 1, "height": 1, "has_alpha": true, "opaque": true})
    );
    assert_eq!(
        image_facts(&webp([83, 192]), "image/webp").unwrap(),
        Some(ImageFacts {
            width: 1,
            height: 1,
            has_alpha: true,
            opaque: false
        })
    );
    assert!(image_facts(b"not a picture", "image/webp").is_err());
    assert_eq!(image_facts(b"x", "audio/wav").unwrap(), None);
}
