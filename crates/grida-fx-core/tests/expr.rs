//! The expression language over small test scopes: templates, the lexer, the grammar, numbers,
//! evaluation, members and indexes, functions, pending values and text, with FX's rules.
//!
//! `MapScope` resolves names from a map, refuses every view method and answers `facts()` from a
//! map. `ViewScope` adds step views: each reference hands out a new view, so identity can be
//! tested.
//!
//! Messages quote values with Python's `repr` (`text::py_repr_str`). Cases whose quoting differs
//! from plain single quotes are checked only once that function is the real one.

use grida_fx_core::expr::{
    self, BinaryOp, Expr, ExprError, Scope, evaluate, finish, parse, render, resolve, template,
};
use grida_fx_core::text::py_repr_str;
use grida_fx_core::val::{Collection, FileContent, FileValue, Pending, Val, Verdict, ViewId};
use grida_fx_core::value::digest;
use indexmap::IndexMap;
use serde_json::{Value, json};
use std::collections::{BTreeSet, HashMap};

const HEX_A: &str = "c3f9c8c283a2b1f2f1896f27a01cbe3cddc0c9d93f752e4639035a0f5b36f6e8";
const HEX_B: &str = "280c3a0354d21e33d2877ed5a2234b37f951bc270ad66004e0731b961bde1c96";

/// Whether `text::py_repr_str` quotes as Python does (it is a stub until its module lands).
fn repr_ready() -> bool {
    py_repr_str("it's") == "\"it's\""
}

// ------------------------------------------------------------------------------------ scopes

struct MapScope {
    names: IndexMap<String, Val>,
    facts: HashMap<String, Val>,
}

impl Scope for MapScope {
    fn root(&mut self, name: &str) -> Result<Val, ExprError> {
        self.names
            .get(name)
            .cloned()
            .ok_or_else(|| ExprError::new(format!("unknown name {}", py_repr_str(name))))
    }

    fn view_member(&mut self, _: ViewId, _: &str) -> Result<Val, ExprError> {
        Err(ExprError::names_a_step())
    }

    fn view_item(&mut self, _: ViewId, _: &Val) -> Result<Val, ExprError> {
        Err(ExprError::names_a_step())
    }

    fn view_every(&mut self, _: ViewId) -> Result<Val, ExprError> {
        Err(ExprError::names_a_step())
    }

    fn view_len(&mut self, _: ViewId) -> Result<usize, ExprError> {
        Err(ExprError::names_a_step())
    }

    fn finish_view(&mut self, _: ViewId) -> Result<Val, ExprError> {
        Err(ExprError::names_a_step())
    }

    fn facts(&mut self, value: &Val) -> Result<Val, ExprError> {
        match value {
            Val::File(f) => Ok(self.facts.get(&f.name).cloned().unwrap_or_else(|| {
                obj(&[
                    ("bytes", Val::Number(f.size as f64)),
                    ("kind", Val::Str(f.kind.clone())),
                ])
            })),
            Val::Missing | Val::Failed(_) => Ok(value.clone()),
            other => Err(ExprError::new(format!(
                "facts() needs a file, not {}",
                other.kind_word()
            ))),
        }
    }
}

fn obj(pairs: &[(&str, Val)]) -> Val {
    Val::Object(
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect(),
    )
}

fn num(x: f64) -> Val {
    Val::Number(x)
}

fn s(text: &str) -> Val {
    Val::Str(text.into())
}

fn list(items: &[Val]) -> Val {
    Val::List(items.to_vec())
}

fn pending(reference: &str) -> Val {
    Val::Pending(Box::new(Pending::of(reference, None)))
}

fn file(digest: &str, kind: &str, name: &str, content: Option<FileContent>) -> Val {
    Val::File(Box::new(FileValue {
        digest: digest.into(),
        kind: kind.into(),
        name: name.into(),
        size: 10,
        key: None,
        content,
        location: None,
    }))
}

fn collection(items: &[(&str, Val)], verdicts: &[(&str, Option<Verdict>)]) -> Val {
    Val::Collection(Box::new(Collection {
        items: items
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect(),
        verdicts: verdicts.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
    }))
}

/// A small environment of every kind of value.
fn scope() -> MapScope {
    let json_content =
        serde_json::from_str::<Value>(r#"{"z": 1, "a": [1, 2], "list": [5, 6]}"#).unwrap();
    let names = [
        ("x", list(&[num(1.0), num(2.0), num(3.0)])),
        ("m", obj(&[("a", num(1.0)), ("b", Val::Null)])),
        ("s", s("abc")),
        ("n", Val::Null),
        ("M", Val::Missing),
        ("F", Val::Failed("a#1".into())),
        ("P", pending("a#1")),
        ("Q", pending("b#1")),
        ("t", Val::Bool(true)),
        ("one", num(1.0)),
        ("two", num(2.0)),
        ("zero", num(0.0)),
        ("empty", list(&[])),
        (
            "C",
            collection(
                &[("k1", num(1.0)), ("k2", num(2.0))],
                &[("k1", Some(Verdict::Accept)), ("k2", None)],
            ),
        ),
        ("E", collection(&[], &[])),
        ("f", file(HEX_A, "image/png", "x/pic.png", None)),
        ("g", file(HEX_A, "text/plain", "other.txt", None)),
        ("h", file(HEX_B, "image/png", "x/pic.png", None)),
        (
            "j",
            file(
                HEX_B,
                "json",
                "d.json",
                Some(FileContent::Json(json_content)),
            ),
        ),
        (
            "tx",
            file(
                HEX_B,
                "text/plain",
                "n.txt",
                Some(FileContent::Text("hé\n".into())),
            ),
        ),
        (
            "L",
            list(&[
                num(1.0),
                s("a"),
                Val::Null,
                Val::Bool(true),
                obj(&[("b", num(2.0)), ("a", num(1.5))]),
            ]),
        ),
        (
            "D",
            obj(&[("z", num(1.0)), ("a", list(&[num(1.0), num(2.0)]))]),
        ),
        ("big", num(1e20)),
        ("parts", list(&[s("a"), pending("c#1")])),
        ("bad", list(&[s("a"), Val::Failed("d#1".into())])),
    ];
    let mut facts = HashMap::new();
    facts.insert(
        "x/pic.png".to_string(),
        obj(&[
            ("bytes", num(10.0)),
            ("kind", s("image/png")),
            ("width", num(3.0)),
            ("height", num(2.0)),
        ]),
    );
    MapScope {
        names: names.into_iter().map(|(k, v)| (k.to_string(), v)).collect(),
        facts,
    }
}

fn eval(source: &str) -> Result<Val, ExprError> {
    let mut scope = scope();
    let expr = parse(source)?;
    evaluate(&expr, &mut scope)
}

fn ok(source: &str) -> Val {
    eval(source).unwrap_or_else(|e| panic!("{source}: {e}"))
}

fn err(source: &str) -> String {
    match eval(source) {
        Ok(v) => panic!("{source}: expected an error, got {v:?}"),
        Err(e) => e.0,
    }
}

fn refs(value: &Val) -> Vec<String> {
    match value {
        Val::Pending(p) => p.refs.iter().cloned().collect(),
        other => panic!("not pending: {other:?}"),
    }
}

fn rendered(source: &str) -> Result<Val, ExprError> {
    let mut scope = scope();
    match template(source)? {
        None => Ok(Val::Str(source.into())),
        Some(t) => render(&t, &mut scope),
    }
}

fn text_of(source: &str) -> String {
    match rendered(source) {
        Ok(Val::Str(text)) => text,
        other => panic!("{source:?}: {other:?}"),
    }
}

fn render_err(source: &str) -> String {
    match rendered(source) {
        Ok(v) => panic!("{source:?}: expected an error, got {v:?}"),
        Err(e) => e.0,
    }
}

// ----------------------------------------------------------------------------------- templates

#[test]
fn template_splitting() {
    assert_eq!(template("plain text }} ok").unwrap(), None);
    assert_eq!(template("${ {1}}").unwrap(), None);
    assert_eq!(text_of("$${{ 1 }}"), "$1");
    assert_eq!(text_of("${{ 'a' }}}"), "a}");
    assert_eq!(text_of("${{ '${{' }}"), "${{");
    assert_eq!(text_of("${{ '}' + '}' }}"), "}}");
    assert_eq!(text_of(" ${{ 1 }}"), " 1");
    assert_eq!(text_of("${{ one }}${{ two }}"), "12");
    assert_eq!(text_of("a ${{ 1 }} b"), "a 1 b");
    assert_eq!(ok("1"), num(1.0));
    assert_eq!(rendered("${{\n 1 +\n 2\n}}").unwrap(), num(3.0));
    assert_eq!(text_of("${{ 1 }}\n"), "1\n");
    // Errors: the first `}}` closes; positions count in the stripped inner text.
    assert_eq!(render_err("${{{ x }}}"), "unexpected '{' at 0 in '{ x'");
    assert_eq!(render_err("${{ }}"), "unexpected '' at 0 in ''");
    assert_eq!(
        render_err("${{   a = b }}"),
        "unexpected '=' at 2 in 'a = b'"
    );
    for source in ["x ${{ 1 }", "x ${{ 1", "${{ 1 }} and ${{"] {
        assert_eq!(
            render_err(source),
            format!("unclosed ${{{{ in {}", py_repr_str(source))
        );
    }
    assert_eq!(render_err("x ${{ 1"), "unclosed ${{ in 'x ${{ 1'");
    if repr_ready() {
        assert_eq!(render_err("${{ '}}' }}"), "unexpected \"'\" at 0 in \"'\"");
    }
}

#[test]
fn every_part_parses_before_anything_evaluates() {
    // A syntax error in a later part wins over an evaluation error in an earlier one.
    assert_eq!(
        render_err("${{ zz }} ${{ ( }}"),
        "unexpected '' at 1 in '('"
    );
    // Every part is evaluated, even after a pending one.
    assert_eq!(render_err("${{ P }} ${{ zz }}"), "unknown name 'zz'");
}

#[test]
fn whole_templates_keep_their_type() {
    assert_eq!(rendered("${{ x }}").unwrap(), ok("x"));
    assert_eq!(rendered("${{ F }}").unwrap(), Val::Failed("a#1".into()));
    assert_eq!(rendered("${{ M }}").unwrap(), Val::Missing);
    assert_eq!(rendered("${{ f }}").unwrap(), ok("f"));
    assert_eq!(refs(&rendered("${{ P }}").unwrap()), ["a#1"]);
}

#[test]
fn mixed_templates_render_text() {
    assert_eq!(text_of("v=${{ M }}"), "v=");
    assert_eq!(text_of("v=${{ n }}"), "v=");
    assert_eq!(text_of("v=${{ t }}"), "v=true");
    assert_eq!(text_of("v=${{ x }}"), "v=[1,2,3]");
    assert_eq!(
        text_of("v=${{ L }}"),
        r#"v=[1,"a",null,true,{"a":1.5,"b":2}]"#
    );
    assert_eq!(text_of("v=${{ D }}"), r#"v={"a":[1,2],"z":1}"#);
    assert_eq!(text_of("v=${{ f }}"), format!("v=sha256:{HEX_A}"));
    assert_eq!(text_of("v=${{ j }}"), r#"v={"z":1,"a":[1,2],"list":[5,6]}"#);
    assert_eq!(text_of("v=${{ tx }}"), "v=hé\n");
    assert_eq!(
        text_of("v=${{ C }}"),
        r#"v={"collection":[["k1",1],["k2",2]]}"#
    );
    // JCS numbers.
    assert_eq!(text_of("v=${{ 0.1 + 0.2 }}"), "v=0.30000000000000004");
    assert_eq!(text_of("v=${{ big * 10 }}"), "v=1e+21");
    assert_eq!(text_of("v=${{ 10000000000000000 }}"), "v=10000000000000000");
    assert_eq!(text_of("v=${{ 1 / 10000000 }}"), "v=1e-7");
    assert_eq!(text_of("v=${{ 1 / 3 }}"), "v=0.3333333333333333");
    assert_eq!(text_of("v=${{ 2/3*3 }}"), "v=2");
    assert_eq!(text_of("v=${{ -1 }}"), "v=-1");
    assert_eq!(text_of("v=${{ 1.0 }}"), "v=1");
    assert_eq!(text_of("v=${{ -0 }}"), "v=0");
}

#[test]
fn mixed_templates_with_pending_or_failed_parts() {
    // A pending part makes the whole string pending, with exact refs.
    let value = rendered("v=${{ P }} and ${{ Q.x }}").unwrap();
    assert_eq!(refs(&value), ["a#1", "b#1"]);
    let Val::Pending(p) = &value else { panic!() };
    let a = Pending::of("a#1", None);
    let qx = Pending::of("b#1", None).derive("field", &[s("x")]).unwrap();
    assert_eq!(
        p.token,
        digest(&json!({"op": "text", "args": [
            "v=", {"pending": a.token}, " and ", {"pending": qx.token}
        ]}))
    );
    // A pending value inside a list part, too (gnode rendered its token into the text).
    assert_eq!(refs(&rendered("v=${{ parts }}").unwrap()), ["c#1"]);
    // FX: a failed part makes the string that failed result, so its reader is blocked.
    assert_eq!(rendered("t=${{ F }}").unwrap(), Val::Failed("a#1".into()));
    assert_eq!(rendered("t=${{ bad }}").unwrap(), Val::Failed("d#1".into()));
    // Pending wins over failed.
    assert_eq!(refs(&rendered("${{ F }}${{ P }}").unwrap()), ["a#1"]);
    // Two textually identical templates over the same references are equal.
    assert_eq!(
        rendered("v=${{ P.x }}").unwrap(),
        rendered("v=${{ P.x }}").unwrap()
    );
    assert_ne!(
        rendered("v=${{ P.x }}").unwrap(),
        rendered("v=${{ P.y }}").unwrap()
    );
}

#[test]
fn resolve_walks_documents() {
    let mut scope = scope();
    let doc = json!({
        "b": "${{ x }}",
        "a": ["no template", "n=${{ len(x) }}", 3, true, null],
        "c": {"${{ k }}": "${{ m.a + 1 }}"},
    });
    let value = resolve(&doc, &mut scope).unwrap();
    let Val::Object(map) = &value else { panic!() };
    assert_eq!(map.keys().collect::<Vec<_>>(), ["b", "a", "c"]);
    assert_eq!(map["b"], ok("x"));
    assert_eq!(
        map["a"],
        list(&[
            s("no template"),
            s("n=3"),
            num(3.0),
            Val::Bool(true),
            Val::Null
        ])
    );
    assert_eq!(map["c"], obj(&[("${{ k }}", num(2.0))]));
    assert_eq!(
        resolve(&json!("x ${{ 1"), &mut scope).unwrap_err().0,
        "unclosed ${{ in 'x ${{ 1'"
    );
}

// --------------------------------------------------------------------------------------- lexer

#[test]
fn lexer_errors() {
    assert_eq!(err("a = b"), "unexpected '=' at 2 in 'a = b'");
    for (source, ch) in [
        ("a & b", "&"),
        ("a | b", "|"),
        ("a ? b", "?"),
        ("a { b", "{"),
        ("a } b", "}"),
        ("a : b", ":"),
        ("a % b", "%"),
        ("a é b", "é"),
    ] {
        assert_eq!(err(source), format!("unexpected '{ch}' at 2 in '{source}'"));
    }
    assert_eq!(err("1e3"), "unexpected 'e3' at 1 in '1e3'");
    assert_eq!(err("0x10"), "unexpected 'x10' at 1 in '0x10'");
    assert_eq!(err(".5"), "unexpected '.' at 0 in '.5'");
    assert_eq!(err("1."), "expected a field name at 2 in '1.'");
    assert_eq!(err("1.2.3"), "expected a field name at 4 in '1.2.3'");
    assert_eq!(err("x.0"), "expected a field name at 2 in 'x.0'");
    // FX: ASCII digits and whitespace only.
    assert_eq!(err("١٢"), "unexpected '١' at 0 in '١٢'");
    assert_eq!(
        err("1 +\u{3000}1"),
        format!(
            "unexpected {} at 3 in {}",
            py_repr_str("\u{3000}"),
            py_repr_str("1 +\u{3000}1")
        )
    );
    if repr_ready() {
        assert_eq!(err("'abc"), "unexpected \"'\" at 0 in \"'abc\"");
        assert_eq!(err("'it''s'"), "unexpected \"'s'\" at 4 in \"'it''s'\"");
        assert_eq!(
            err("1 +\u{3000}1"),
            "unexpected '\\u3000' at 3 in '1 +\\u30001'"
        );
    }
}

#[test]
fn strings_have_no_c_escapes() {
    assert_eq!(ok(r"'a\nb'"), s("anb"));
    assert_eq!(ok(r"'\t'"), s("t"));
    assert_eq!(ok(r"'\u0041'"), s("u0041"));
    assert_eq!(ok(r"'a\\b'"), s(r"a\b"));
    assert_eq!(ok(r"'it\'s'"), s("it's"));
    assert_eq!(ok("\"it's\""), s("it's"));
    assert_eq!(ok("'a\nb'"), s("a\nb"));
}

#[test]
fn keywords() {
    assert_eq!(ok("true"), Val::Bool(true));
    assert_eq!(ok("null"), Val::Null);
    assert_eq!(err("True"), "unknown name 'True'");
    assert_eq!(err("NULL"), "unknown name 'NULL'");
    let mut scope = scope();
    scope
        .names
        .insert("k".into(), obj(&[("true", num(1.0)), ("null", num(2.0))]));
    let value = evaluate(&parse("k.true + k.null").unwrap(), &mut scope).unwrap();
    assert_eq!(value, num(3.0));
}

// ------------------------------------------------------------------------------------- grammar

#[test]
fn precedence() {
    let same = |a: &str, b: &str| assert_eq!(parse(a).unwrap(), parse(b).unwrap(), "{a} vs {b}");
    same("-a.b", "-(a.b)");
    same("-x[0]", "-(x[0])");
    same("!1 == false", "(!1) == false");
    same("- -1 + 1", "(-(-1)) + 1");
    same("a ?? b || c", "a ?? (b || c)");
    same("a || b ?? c", "(a || b) ?? c");
    same("a ?? b ?? c", "a ?? (b ?? c)");
    same("1 < 2 < 3", "(1 < 2) < 3");
    same("a + b * c - d / e", "(a + (b * c)) - (d / e)");
    same("a == b && c != d || e", "((a == b) && (c != d)) || e");
    same("x . y", "x.y");
    same("x .*", "x.*");
    same("x. *", "x.*");
    same("x ['a']", "x['a']");
    assert!(matches!(
        parse("a ?? b").unwrap(),
        Expr::Binary(BinaryOp::Coalesce, _, _)
    ));
    assert_eq!(ok("-m.a"), num(-1.0));
    assert_eq!(ok("-x[0]"), num(-1.0));
    assert_eq!(ok("!1 == false"), Val::Bool(true));
    assert_eq!(ok("- -1 + 1"), num(2.0));
    assert_eq!(ok("-(1) * 2"), num(-2.0));
    assert_eq!(err("1 < 2 < 3"), "< needs numbers, not a boolean");
}

#[test]
fn grammar_errors() {
    assert_eq!(err("true(1)"), "unexpected '(' at 4 in 'true(1)'");
    assert_eq!(err("len"), "unknown name 'len'");
    let functions = "accepted, concat, contains, digest, facts, join, len, lookup, max, min, stem";
    assert_eq!(
        err("Len(x)"),
        format!(
            "Len() is not an expression function; write a node for it (functions: {functions})"
        )
    );
    assert_eq!(
        err("upper(s)"),
        format!(
            "upper() is not an expression function; write a node for it (functions: {functions})"
        )
    );
    assert_eq!(err("[1]"), "unexpected '[' at 0 in '[1]'");
    assert_eq!(err("min([])"), "unexpected '[' at 4 in 'min([])'");
    assert_eq!(err("len(x,)"), "unexpected ')' at 6 in 'len(x,)'");
    assert_eq!(err("len("), "unexpected '' at 4 in 'len('");
    assert_eq!(err("x[0"), "expected ']' at 3 in 'x[0'");
    assert_eq!(err("(1"), "expected ')' at 2 in '(1'");
    assert_eq!(err("1 2"), "unexpected '2' at 2 in '1 2'");
    assert_eq!(err("a ?"), "unexpected '?' at 2 in 'a ?'");
}

#[test]
fn postfix_applies_to_calls_and_parentheses() {
    assert_eq!(ok("facts(f).width"), num(3.0));
    assert_eq!(ok("(x)[0]"), num(1.0));
    assert_eq!(ok("(m).a"), num(1.0));
    assert_eq!(ok("lookup(D, 'a')[1]"), num(2.0));
}

// ------------------------------------------------------------------------------------- numbers

#[test]
fn number_literals() {
    assert_eq!(ok("1.50"), num(1.5));
    assert_eq!(ok("9007199254740992"), num(9007199254740992.0));
    let beyond = |text: &str| {
        format!(
            "{text} is beyond the integers a number holds exactly (2^53 = 9007199254740992); \
             quote it if it is an identifier, and give its input the string type"
        )
    };
    assert_eq!(err("9007199254740993"), beyond("9007199254740993"));
    assert_eq!(err("12345678901234567891"), beyond("12345678901234567891"));
    assert_eq!(
        render_err("v=${{ 9007199254740993 }}"),
        beyond("9007199254740993")
    );
    assert_eq!(err("007"), "007 is not written in canonical form; write 7");
}

#[test]
fn arithmetic() {
    assert_eq!(ok("'a' + 'b'"), s("ab"));
    assert_eq!(err("'a' + 1"), "+ needs numbers, not text");
    assert_eq!(err("1 + 'a'"), "+ needs numbers, not text");
    assert_eq!(err("x + x"), "+ needs numbers, not a list");
    assert_eq!(err("true + 1"), "+ needs numbers, not a boolean");
    assert_eq!(err("M + 1"), "+ needs numbers, not a missing value");
    assert_eq!(err("1 - n"), "- needs numbers, not null");
    assert_eq!(err("-s"), "- needs numbers, not text");
    assert_eq!(ok("6 / 3"), num(2.0));
    assert_eq!(ok("7 / 2"), num(3.5));
    assert_eq!(ok("2 * 3 - 1"), num(5.0));
    assert_eq!(err("1 / 0"), "division by zero");
    assert_eq!(err("1 / zero"), "division by zero");
    assert_eq!(err("1 / -0"), "division by zero");
    assert_eq!(ok("0.1 + 0.2"), num(0.1 + 0.2));
    // Results must be finite.
    assert_eq!(
        err(
            "big * big * big * big * big * big * big * big * big * big * big * big * big * big * big * big"
        ),
        "* of a non-finite number"
    );
    // FX: unary minus gives an ordinary number, so it indexes from the end.
    assert_eq!(ok("x[-1]"), num(3.0));
    assert_eq!(ok("x[0 - 1]"), num(3.0));
    assert_eq!(ok("-(-5)"), num(5.0));
    assert_eq!(ok("-0"), num(0.0));
}

#[test]
fn comparisons() {
    assert_eq!(ok("'B' < 'a'"), Val::Bool(true));
    assert_eq!(ok("'b' >= 'b'"), Val::Bool(true));
    assert_eq!(ok("1 <= 1.0"), Val::Bool(true));
    assert_eq!(ok("2 > 10"), Val::Bool(false));
    assert_eq!(err("'a' < 1"), "< needs numbers, not text");
    assert_eq!(err("1 < 'a'"), "< needs numbers, not text");
    assert_eq!(err("null < 1"), "< needs numbers, not null");
    assert_eq!(err("1 >= x"), ">= needs numbers, not a list");
}

// ---------------------------------------------------------------------------------- evaluation

#[test]
fn operand_returning_and_short_circuit() {
    assert_eq!(ok("0 || 'b'"), s("b"));
    assert_eq!(ok("1 || 'b'"), num(1.0));
    assert_eq!(ok("0 && 'b'"), num(0.0));
    assert_eq!(ok("1 && 'b'"), s("b"));
    assert_eq!(ok("'' || ''"), s(""));
    // Short-circuited operands are never evaluated.
    assert_eq!(ok("null && x.y.z"), Val::Null);
    assert_eq!(ok("1 || unknown"), num(1.0));
    assert_eq!(ok("false && zz.y.z"), Val::Bool(false));
    assert_eq!(ok("M || 'fallback'"), s("fallback"));
    assert_eq!(ok("F || 'fallback'"), Val::Failed("a#1".into()));
    assert_eq!(ok("E || 'x'"), ok("E"));
    assert_eq!(err("0 || unknown"), "unknown name 'unknown'");
}

#[test]
fn coalescing() {
    assert_eq!(ok("0 ?? 3"), num(0.0));
    assert_eq!(ok("'' ?? 3"), s(""));
    assert_eq!(ok("F ?? 3"), Val::Failed("a#1".into()));
    assert_eq!(ok("null ?? 3"), num(3.0));
    assert_eq!(ok("M ?? 3"), num(3.0));
    assert_eq!(ok("M.x.y ?? 'd'"), s("d"));
    assert_eq!(ok("1 ?? unknown"), num(1.0));
    assert_eq!(ok("n ?? M ?? 4"), num(4.0));
}

#[test]
fn equality() {
    assert_eq!(ok("1 == 1.0"), Val::Bool(true));
    // FX: booleans are not numbers.
    assert_eq!(ok("true == 1"), Val::Bool(false));
    assert_eq!(ok("true != 1"), Val::Bool(true));
    assert_eq!(ok("'1' == 1"), Val::Bool(false));
    assert_eq!(ok("x == x"), Val::Bool(true));
    assert_eq!(ok("x == concat(x)"), Val::Bool(true));
    assert_eq!(ok("m == m"), Val::Bool(true));
    // Files are equal when their digests are.
    assert_eq!(ok("f == g"), Val::Bool(true));
    assert_eq!(ok("f == h"), Val::Bool(false));
    assert_eq!(ok("M == M"), Val::Bool(true));
    assert_eq!(ok("n == M"), Val::Bool(false));
    assert_eq!(ok("F == F"), Val::Bool(true));
    assert_eq!(ok("C == C"), Val::Bool(true));
    assert_eq!(ok("0.1 + 0.2 == 0.3"), Val::Bool(false));
}

#[test]
fn pending_propagation() {
    let a = ["a#1"];
    // Unary, member, every, index, facts, accepted.
    assert_eq!(refs(&ok("!P")), a);
    assert_eq!(refs(&ok("-P")), a);
    assert_eq!(refs(&ok("P.x")), a);
    assert_eq!(refs(&ok("P.*")), a);
    assert_eq!(refs(&ok("P[0]")), a);
    assert_eq!(refs(&ok("x[P]")), a);
    assert_eq!(refs(&ok("facts(P)")), a);
    assert_eq!(refs(&ok("accepted(P)")), a);
    // Binary operators, except the short circuits.
    assert_eq!(ok("false && P"), Val::Bool(false));
    assert_eq!(refs(&ok("true && P")), a);
    assert_eq!(refs(&ok("P && 1")), a);
    assert_eq!(refs(&ok("P || 1")), a);
    assert_eq!(refs(&ok("P ?? 3")), a);
    assert_eq!(ok("3 ?? P"), num(3.0));
    assert_eq!(refs(&ok("P + Q")), ["a#1", "b#1"]);
    assert_eq!(refs(&ok("P == 1")), a);
    assert_eq!(refs(&ok("1 < Q")), ["b#1"]);
    // ?? with a pending left evaluates its right side.
    assert_eq!(refs(&ok("P ?? Q")), ["a#1", "b#1"]);
    assert_eq!(err("P ?? zz"), "unknown name 'zz'");
    // Functions: after the arity check.
    assert_eq!(refs(&ok("len(P)")), a);
    assert_eq!(refs(&ok("concat(P, x)")), a);
    assert_eq!(refs(&ok("lookup(m, P)")), a);
    assert_eq!(err("len(P, 1)"), "len() takes 1 values");
    // Tokens are exact and stable.
    let p = Pending::of("a#1", None);
    let Val::Pending(field) = ok("P.x") else {
        panic!()
    };
    assert_eq!(
        field.token,
        digest(&json!({"op": "field", "of": p.token, "extra": ["x"]}))
    );
    let Val::Pending(sum) = ok("P + 1") else {
        panic!()
    };
    assert_eq!(
        sum.token,
        digest(&json!({"op": "+", "args": [{"pending": p.token}, 1]}))
    );
    let Val::Pending(index) = ok("x[P]") else {
        panic!()
    };
    assert_eq!(
        index.token,
        digest(&json!({"op": "index", "args": [[1, 2, 3], {"pending": p.token}]}))
    );
    let Val::Pending(neg) = ok("-P") else {
        panic!()
    };
    assert_eq!(
        neg.token,
        digest(&json!({"op": "-", "of": p.token, "extra": []}))
    );
    assert_eq!(ok("P.x"), ok("P.x"));
    assert_ne!(ok("P.x"), ok("P.y"));
}

#[test]
fn nested_pending_in_content_reading_functions() {
    // `parts` is a list holding a pending value.
    assert_eq!(refs(&ok("join(parts, ',')")), ["c#1"]);
    assert_eq!(refs(&ok("digest(parts)")), ["c#1"]);
    assert_eq!(refs(&ok("contains(parts, 'a')")), ["c#1"]);
    assert_eq!(refs(&ok("parts == x")), ["c#1"]);
    // Shape-only functions see the list as it is.
    assert_eq!(ok("len(parts)"), num(2.0));
    assert_eq!(ok("parts[0]"), s("a"));
    assert_eq!(refs(&ok("parts[1]")), ["c#1"]);
}

// ---------------------------------------------------------------------------- member and index

#[test]
fn members_and_indexes_by_value() {
    // Objects.
    assert_eq!(ok("m.a"), num(1.0));
    assert_eq!(ok("m.b"), Val::Null);
    assert_eq!(err("m.zz"), "no field 'zz'");
    assert_eq!(ok("m['a']"), num(1.0));
    assert_eq!(err("m['k']"), "no entry 'k'");
    assert_eq!(err("m[1]"), "no entry 1");
    assert_eq!(err("m[true]"), "no entry True");
    assert_eq!(err("m[n]"), "no entry None");
    // Lists.
    assert_eq!(err("x.name"), "a list has no field 'name'");
    assert_eq!(ok("x[2]"), num(3.0));
    assert_eq!(ok("x[-3]"), num(1.0));
    assert_eq!(ok("x[1.0]"), num(2.0));
    assert_eq!(err("x[5]"), "index 5 is outside a list of 3");
    assert_eq!(err("x[-4]"), "index -4 is outside a list of 3");
    assert_eq!(
        err(&format!("x[1{}.0]", "0".repeat(30))),
        "index 1e+30 is outside a list of 3"
    );
    assert_eq!(
        err("x['a']"),
        "a list index must be a whole number, not text"
    );
    assert_eq!(
        err("x[true]"),
        "a list index must be a whole number, not a boolean"
    );
    assert_eq!(
        err("x[1.5]"),
        "a list index must be a whole number, not 1.5"
    );
    // Text, numbers, booleans, null.
    assert_eq!(err("s.x"), "text has no field 'x'");
    assert_eq!(err("s[0]"), "text cannot be indexed");
    assert_eq!(err("(1).x"), "a number has no field 'x'");
    assert_eq!(err("true.x"), "a boolean has no field 'x'");
    assert_eq!(err("null.x"), "null has no field 'x'");
    assert_eq!(err("(1)[0]"), "a number cannot be indexed");
    assert_eq!(err("n[0]"), "null cannot be indexed");
    // `.*` applies only to views.
    assert_eq!(err("x.*"), ".* applies to a repeated step");
    assert_eq!(err("M.*"), ".* applies to a repeated step");
    assert_eq!(err("F.*"), ".* applies to a repeated step");
}

#[test]
fn files_missing_failed() {
    // A JSON-object file reads into its content.
    assert_eq!(ok("j.z"), num(1.0));
    assert_eq!(ok("j['z']"), num(1.0));
    assert_eq!(ok("j.list[1]"), num(6.0));
    assert_eq!(err("j.digest"), "d.json has no field 'digest'");
    // Other files have digest, kind and key.
    assert_eq!(ok("f.digest"), s(HEX_A));
    assert_eq!(ok("f.kind"), s("image/png"));
    assert_eq!(ok("f.key"), Val::Null);
    assert_eq!(
        err("f.width"),
        "a file has no field 'width'; use facts(file).width"
    );
    assert_eq!(err("f[0]"), "x/pic.png cannot be indexed");
    assert_eq!(err("tx[0]"), "n.txt cannot be indexed");
    // Missing and failed values stay themselves.
    assert_eq!(ok("M.x"), Val::Missing);
    assert_eq!(ok("M[0]"), Val::Missing);
    assert_eq!(ok("M.a.b[3]"), Val::Missing);
    assert_eq!(ok("F.x"), Val::Failed("a#1".into()));
    assert_eq!(ok("F[0]"), Val::Failed("a#1".into()));
}

#[test]
fn collections() {
    let mut scope = scope();
    let docs = collection(
        &[
            ("ada", obj(&[("t", s("A")), ("o", num(1.0))])),
            ("bo", obj(&[("t", s("B"))])),
            ("cy", Val::Missing),
        ],
        &[("ada", Some(Verdict::Accept)), ("zz", None)],
    );
    scope.names.insert("docs".into(), docs);
    let mut ev = |source: &str| evaluate(&parse(source).unwrap(), &mut scope);
    // `.name` maps over the elements, dropping missing fields; verdicts are kept.
    assert_eq!(
        ev("docs.t").unwrap(),
        collection(
            &[("ada", s("A")), ("bo", s("B"))],
            &[("ada", Some(Verdict::Accept)), ("zz", None)]
        )
    );
    // An element without the field is an error, as `.name` on it is.
    assert_eq!(ev("docs.o").unwrap_err().0, "no field 'o'");
    // `[key]` first, then positions.
    assert_eq!(ev("docs['bo'].t").unwrap(), s("B"));
    assert_eq!(ev("docs[0].t").unwrap(), s("A"));
    assert_eq!(ev("docs[-1]").unwrap(), Val::Missing);
    assert_eq!(ev("len(docs)").unwrap(), num(3.0));
    // FX: out of range is `no instance N`, not a crash.
    assert_eq!(ev("docs[3]").unwrap_err().0, "no instance 3");
    assert_eq!(ev("docs[true]").unwrap_err().0, "no instance True");
    assert_eq!(ev("docs[1.5]").unwrap_err().0, "no instance 1.5");
    assert_eq!(ev("docs[-4]").unwrap_err().0, "no instance -4");
    assert_eq!(ev("docs['zz']").unwrap_err().0, "no instance 'zz'");
    assert_eq!(ev("docs.*").unwrap_err().0, ".* applies to a repeated step");
}

// ----------------------------------------------------------------------------------- functions

#[test]
fn arity() {
    assert_eq!(err("facts()"), "facts() takes 1 values");
    assert_eq!(err("facts(f, f)"), "facts() takes 1 values");
    assert_eq!(err("lookup(m)"), "lookup() takes 2 values");
    assert_eq!(err("min()"), "min() takes 1 or more values");
    assert_eq!(err("max()"), "max() takes 1 or more values");
    assert_eq!(err("concat()"), "concat() takes 1 or more values");
    assert_eq!(err("join(x)"), "join() takes 2 values");
    assert_eq!(err("contains(x)"), "contains() takes 2 values");
    assert_eq!(err("stem()"), "stem() takes 1 values");
    assert_eq!(err("digest(1, 2)"), "digest() takes 1 values");
    assert_eq!(err("len(1, 2)"), "len() takes 1 values");
    assert_eq!(err("accepted()"), "accepted() takes 1 values");
    // Arguments are evaluated, left to right, before the arity check.
    assert_eq!(err("len(zz, 1)"), "unknown name 'zz'");
    assert_eq!(err("len(1, zz)"), "unknown name 'zz'");
}

#[test]
fn facts_and_accepted() {
    assert_eq!(ok("facts(f).height"), num(2.0));
    assert_eq!(
        ok("facts(g)"),
        obj(&[("bytes", num(10.0)), ("kind", s("text/plain"))])
    );
    assert_eq!(ok("facts(M)"), Val::Missing);
    assert_eq!(ok("facts(F)"), Val::Failed("a#1".into()));
    assert_eq!(err("facts(1)"), "facts() needs a file, not a number");
    assert_eq!(err("facts(x)"), "facts() needs a file, not a list");
    // accepted(): undecided verdicts make a pending value, with no refs when every item is known.
    let Val::Pending(p) = ok("accepted(C)") else {
        panic!()
    };
    assert!(p.refs.is_empty());
    assert_eq!(
        p.token,
        digest(&json!({"accepted": {"collection": [["k1", 1], ["k2", 2]]}}))
    );
    assert_eq!(ok("accepted(E)"), ok("E"));
    assert_eq!(
        err("accepted(x)"),
        "accepted() needs a collection of a repeated step"
    );
    assert_eq!(
        err("accepted(M)"),
        "accepted() needs a collection of a repeated step"
    );
}

#[test]
fn accepted_keeps_accepted_and_unjudged_items() {
    let decided = Collection {
        items: vec![
            ("a".into(), num(1.0)),
            ("b".into(), num(2.0)),
            ("c".into(), num(3.0)),
            ("d".into(), num(4.0)),
        ],
        verdicts: [
            ("a".to_string(), Some(Verdict::Accept)),
            ("b".to_string(), Some(Verdict::Reject)),
            ("c".to_string(), Some(Verdict::Failed)),
        ]
        .into_iter()
        .collect(),
    };
    assert_eq!(
        expr::functions::accepted(&decided).unwrap(),
        collection(
            &[("a", num(1.0)), ("d", num(4.0))],
            &[("a", Some(Verdict::Accept))]
        )
    );
    let undecided = Collection {
        items: vec![("a".into(), pending("r['a']#1")), ("b".into(), num(2.0))],
        verdicts: [("a".to_string(), None)].into_iter().collect(),
    };
    assert_eq!(
        refs(&expr::functions::accepted(&undecided).unwrap()),
        ["r['a']#1"]
    );
}

#[test]
fn lookup_and_contains_use_fx_equality() {
    assert_eq!(ok("lookup(m, 'a')"), num(1.0));
    assert_eq!(ok("lookup(m, 'b')"), Val::Null);
    assert_eq!(err("lookup(x, 'a')"), "lookup() needs a table");
    assert_eq!(err("lookup(j, 'z')"), "lookup() needs a table");
    assert_eq!(err("lookup(m, 'zz')"), "lookup(): no entry 'zz'");
    assert_eq!(err("lookup(m, 1)"), "lookup(): no entry 1");
    assert_eq!(err("lookup(m, true)"), "lookup(): no entry True");
    // FX: an unhashable key finds nothing (gnode crashed).
    assert_eq!(err("lookup(m, x)"), "lookup(): no entry [1, 2, 3]");
    assert_eq!(
        err("lookup(m, m)"),
        "lookup(): no entry {'a': 1, 'b': None}"
    );

    assert_eq!(ok("contains('abc', 'b')"), Val::Bool(true));
    assert_eq!(ok("contains('abc', '')"), Val::Bool(true));
    assert_eq!(ok("contains('abc', 'd')"), Val::Bool(false));
    assert_eq!(ok("contains(x, 1)"), Val::Bool(true));
    assert_eq!(ok("contains(x, 1.0)"), Val::Bool(true));
    assert_eq!(ok("contains(L, true)"), Val::Bool(true));
    assert_eq!(ok("contains(x, true)"), Val::Bool(false));
    assert_eq!(ok("contains(m, 'a')"), Val::Bool(true));
    assert_eq!(ok("contains(m, 'zz')"), Val::Bool(false));
    assert_eq!(ok("contains(m, x)"), Val::Bool(false));
    assert_eq!(ok("contains(m, 1)"), Val::Bool(false));
    assert_eq!(err("contains('abc', 1)"), "contains() of text");
    assert_eq!(err("contains(1, 1)"), "contains() of a number");
    assert_eq!(err("contains(C, 1)"), "contains() of a collection");
}

#[test]
fn min_max_len_concat_join_stem() {
    assert_eq!(ok("min(3, 1, 2)"), num(1.0));
    assert_eq!(ok("max(x)"), num(3.0));
    assert_eq!(ok("max(1.5, 2)"), num(2.0));
    assert_eq!(ok("min(-1)"), num(-1.0));
    assert_eq!(err("min(x, 0)"), "min needs numbers, not a list");
    assert_eq!(err("min(empty)"), "min() of nothing");
    assert_eq!(err("max('a')"), "max needs numbers, not text");
    assert_eq!(err("min(C)"), "min needs numbers, not a collection");
    assert_eq!(err("max(1, true)"), "max needs numbers, not a boolean");

    assert_eq!(ok("len(s)"), num(3.0));
    assert_eq!(ok("len('héllo')"), num(5.0));
    assert_eq!(ok("len(x)"), num(3.0));
    assert_eq!(ok("len(m)"), num(2.0));
    assert_eq!(ok("len(C)"), num(2.0));
    assert_eq!(ok("len(j)"), num(3.0));
    assert_eq!(ok("len(tx)"), num(3.0));
    assert_eq!(err("len(f)"), "len() of a image/png file");
    assert_eq!(err("len(1)"), "len() of a number");
    assert_eq!(err("len(null)"), "len() of null");
    assert_eq!(err("len(M)"), "len() of a missing value");
    assert_eq!(err("len(F)"), "len() of a failed result");

    assert_eq!(ok("concat(x, x)"), ok("concat(x, concat(x))"));
    assert_eq!(ok("len(concat(x, x, empty))"), num(6.0));
    assert_eq!(ok("concat('a', 'b', 'c')"), s("abc"));
    assert_eq!(ok("concat('a')"), s("a"));
    assert_eq!(
        err("concat(x, 'a')"),
        "concat() joins lists with lists or text with text"
    );
    assert_eq!(
        err("concat(C, C)"),
        "concat() joins lists with lists or text with text"
    );

    assert_eq!(ok("join(x, ', ')"), s("1, 2, 3"));
    assert_eq!(ok("join(L, '|')"), s(r#"1|a||true|{"a":1.5,"b":2}"#));
    assert_eq!(ok("join(empty, ',')"), s(""));
    assert_eq!(err("join(s, ',')"), "join() takes a list and a separator");
    assert_eq!(err("join(x, 1)"), "join() takes a list and a separator");
    assert_eq!(err("join(C, ',')"), "join() takes a list and a separator");
    // FX: a failed element blocks the reader, as in a mixed template.
    assert_eq!(ok("join(bad, ',')"), Val::Failed("d#1".into()));

    assert_eq!(ok("stem(f)"), s("pic"));
    assert_eq!(ok("stem('x/pic.png')"), s("pic"));
    assert_eq!(ok("stem('a/b/c.tar.gz')"), s("c.tar"));
    assert_eq!(ok("stem('.bashrc')"), s(""));
    assert_eq!(ok("stem('a/b/')"), s(""));
    assert_eq!(ok("stem('noext')"), s("noext"));
    assert_eq!(
        err("stem(1)"),
        "stem() needs a file or a path, not a number"
    );
    assert_eq!(
        err("stem(M)"),
        "stem() needs a file or a path, not a missing value"
    );
}

#[test]
fn digest_is_jcs() {
    assert_eq!(ok("digest(1)"), s("6b86b273ff34fce1"));
    // FX: one number type.
    assert_eq!(ok("digest(1.0)"), ok("digest(1)"));
    assert_eq!(ok("digest(2 / 2)"), ok("digest(1)"));
    let want = &digest(&json!({"a": 1, "b": null}))[..16];
    assert_eq!(ok("digest(m)"), s(want));
    let file_digest = &digest(&json!({"file": HEX_A}))[..16];
    assert_eq!(ok("digest(f)"), s(file_digest));
    assert_eq!(ok("digest(f)"), ok("digest(g)"));
    assert_eq!(ok("digest(M)"), s(&digest(&json!({"missing": true}))[..16]));
    assert_eq!(refs(&ok("digest(P)")), ["a#1"]);
}

// --------------------------------------------------------------------------------------- views

#[derive(Clone)]
enum View {
    /// A node step: `outputs` is pending on `id`.
    Node(String),
    /// A repeat: key → node id.
    Repeat(Vec<(String, String)>),
    /// A `.*` result over keyed values.
    Every(Vec<(String, Val)>),
}

/// A scope with step views: `node`, `rep` (a repeat of `ada` and `bo`) and `done` (a repeat whose
/// results exist). Each reference hands out a new view.
struct ViewScope {
    views: Vec<View>,
}

impl ViewScope {
    fn new() -> Self {
        ViewScope { views: Vec::new() }
    }

    fn hand(&mut self, view: View) -> Val {
        self.views.push(view);
        Val::View(ViewId(self.views.len() - 1))
    }
}

impl Scope for ViewScope {
    fn root(&mut self, name: &str) -> Result<Val, ExprError> {
        match name {
            "node" => Ok(self.hand(View::Node("node#1".into()))),
            "rep" => Ok(self.hand(View::Repeat(vec![
                ("ada".into(), "rep['ada']#1".into()),
                ("bo".into(), "rep['bo']#1".into()),
            ]))),
            "done" => Ok(self.hand(View::Every(vec![
                ("ada".into(), obj(&[("text", s("A"))])),
                ("bo".into(), obj(&[("text", s("B"))])),
            ]))),
            "P" => Ok(pending("p#1")),
            _ => Err(ExprError::new(format!(
                "unknown name {}",
                py_repr_str(name)
            ))),
        }
    }

    fn view_member(&mut self, view: ViewId, name: &str) -> Result<Val, ExprError> {
        match self.views[view.0].clone() {
            View::Node(id) if name == "outputs" => Ok(pending(&id)),
            View::Node(_) => Err(ExprError::new(format!(
                "a step has outputs, facts and take, not {}",
                py_repr_str(name)
            ))),
            View::Repeat(children) => {
                let items = children
                    .into_iter()
                    .map(|(key, id)| (key, self.hand(View::Node(id))))
                    .collect();
                let every = self.hand(View::Every(items));
                expr::member(self, every, name)
            }
            View::Every(items) => {
                let mut mapped = Vec::new();
                for (key, value) in items {
                    let m = expr::member(self, value, name)?;
                    if m != Val::Missing {
                        mapped.push((key, m));
                    }
                }
                Ok(self.hand(View::Every(mapped)))
            }
        }
    }

    fn view_item(&mut self, view: ViewId, index: &Val) -> Result<Val, ExprError> {
        match self.views[view.0].clone() {
            View::Repeat(children) => {
                let key = index.key_text()?;
                match children.into_iter().find(|(k, _)| *k == key) {
                    Some((_, id)) => Ok(self.hand(View::Node(id))),
                    None => Err(ExprError::new(format!(
                        "no instance [{}]",
                        py_repr_str(&key)
                    ))),
                }
            }
            _ => Err(ExprError::new("a step cannot be indexed")),
        }
    }

    fn view_item_pending(
        &mut self,
        view: ViewId,
        index: &Pending,
    ) -> Option<Result<Val, ExprError>> {
        // A repeat answers as the step scope does; every other view is left to the evaluator.
        let View::Repeat(children) = &self.views[view.0] else {
            return None;
        };
        let mut refs = index.refs.clone();
        refs.extend(children.iter().map(|(_, id)| id.clone()));
        let token = json!({"index": index.token});
        Some(Ok(Val::Pending(Box::new(Pending::new(refs, &token)))))
    }

    fn view_every(&mut self, view: ViewId) -> Result<Val, ExprError> {
        match self.views[view.0].clone() {
            View::Repeat(children) => {
                let items = children
                    .into_iter()
                    .map(|(key, id)| (key, self.hand(View::Node(id))))
                    .collect();
                Ok(self.hand(View::Every(items)))
            }
            _ => Err(ExprError::new(".* applies to a repeated step")),
        }
    }

    fn view_len(&mut self, view: ViewId) -> Result<usize, ExprError> {
        match &self.views[view.0] {
            View::Repeat(children) => Ok(children.len()),
            View::Every(items) => Ok(items.len()),
            View::Node(_) => Err(ExprError::new("len() of a step")),
        }
    }

    fn finish_view(&mut self, view: ViewId) -> Result<Val, ExprError> {
        match self.views[view.0].clone() {
            View::Every(items) => {
                if items.iter().any(|(_, v)| matches!(v, Val::View(_))) {
                    return Err(ExprError::new(
                        "name what to take from each instance, e.g. .outputs.image",
                    ));
                }
                Ok(Val::Collection(Box::new(Collection {
                    items,
                    verdicts: IndexMap::new(),
                })))
            }
            _ => Err(ExprError::names_a_step()),
        }
    }

    fn facts(&mut self, value: &Val) -> Result<Val, ExprError> {
        Err(ExprError::new(format!(
            "facts() needs a file, not {}",
            value.kind_word()
        )))
    }
}

fn view_eval(source: &str) -> Result<Val, ExprError> {
    let mut scope = ViewScope::new();
    let value = evaluate(&parse(source).unwrap(), &mut scope)?;
    finish(&mut scope, value)
}

fn view_render(source: &str) -> Result<Val, ExprError> {
    let mut scope = ViewScope::new();
    let t = template(source).unwrap().unwrap();
    let value = render(&t, &mut scope)?;
    finish(&mut scope, value)
}

#[test]
fn views_are_delegated_and_finished() {
    let names_a_step = ExprError::names_a_step();
    assert_eq!(refs(&view_eval("node.outputs.image").unwrap()), ["node#1"]);
    assert_eq!(view_eval("node").unwrap_err(), names_a_step);
    assert_eq!(
        view_eval("node.foo").unwrap_err().0,
        "a step has outputs, facts and take, not 'foo'"
    );
    assert_eq!(
        refs(&view_eval("rep['bo'].outputs.text").unwrap()),
        ["rep['bo']#1"]
    );
    assert_eq!(view_eval("rep['zz']").unwrap_err().0, "no instance ['zz']");
    // A `.*` result finishes into a collection.
    assert_eq!(
        view_eval("done.text").unwrap(),
        collection(&[("ada", s("A")), ("bo", s("B"))], &[])
    );
    assert_eq!(
        view_eval("rep.*").unwrap_err().0,
        "name what to take from each instance, e.g. .outputs.image"
    );
    // Views flow through operators before finishing.
    assert_eq!(view_eval("!node").unwrap(), Val::Bool(false));
    assert_eq!(view_eval("node && 1").unwrap(), num(1.0));
    assert_eq!(view_eval("len(rep)").unwrap(), num(2.0));
    assert_eq!(view_eval("len(rep.*)").unwrap(), num(2.0));
    assert_eq!(view_eval("rep ?? 1").unwrap_err(), names_a_step);
    // Two references are two views: they compare by identity.
    assert_eq!(view_eval("node == node").unwrap(), Val::Bool(false));
    assert_eq!(view_eval("node != node").unwrap(), Val::Bool(true));
    // A `.*` result compares as its collection.
    assert_eq!(
        view_eval("done.text == done.text").unwrap(),
        Val::Bool(true)
    );
    // Type errors name a step.
    assert_eq!(
        view_eval("node + 1").unwrap_err().0,
        "+ needs numbers, not a step"
    );
    assert_eq!(
        view_eval("-node").unwrap_err().0,
        "- needs numbers, not a step"
    );
    assert_eq!(
        view_eval("contains(rep, 1)").unwrap_err().0,
        "contains() of a step"
    );
    assert_eq!(view_eval("x.*").unwrap_err().0, "unknown name 'x'");
    assert_eq!(
        view_eval("node.*").unwrap_err().0,
        ".* applies to a repeated step"
    );
}

#[test]
fn views_next_to_pending_values_and_in_text() {
    let names_a_step = ExprError::names_a_step();
    // gnode crashed on these; FX refuses them.
    assert_eq!(view_eval("P && node").unwrap_err(), names_a_step);
    assert_eq!(view_eval("digest(node)").unwrap_err(), names_a_step);
    // A pending index goes to the scope: a repeat waits on the index and every instance, and
    // then derives as any pending value does.
    let picked = view_eval("rep[P].outputs.text").unwrap();
    assert_eq!(refs(&picked), ["p#1", "rep['ada']#1", "rep['bo']#1"]);
    let Val::Pending(p) = &picked else { panic!() };
    let indexed = digest(&json!({"index": Pending::of("p#1", None).token}));
    let outputs = digest(&json!({"op": "field", "of": indexed, "extra": ["outputs"]}));
    assert_eq!(
        p.token,
        digest(&json!({"op": "field", "of": outputs, "extra": ["text"]}))
    );
    // A view the scope does not answer for is finished first, as before.
    assert_eq!(view_eval("node[P]").unwrap_err(), names_a_step);
    assert_eq!(refs(&view_eval("done.text[P]").unwrap()), ["p#1"]);
    // MapScope keeps the default: an index into a pending value or by one derives.
    assert_eq!(refs(&ok("x[P]")), ["a#1"]);
    assert_eq!(view_render("v=${{ node }}").unwrap_err(), names_a_step);
    // A `.*` result is its collection there.
    assert_eq!(refs(&view_eval("P ?? done.text").unwrap()), ["p#1"]);
    assert_eq!(
        view_render("v=${{ done.text }}").unwrap(),
        s(r#"v={"collection":[["ada","A"],["bo","B"]]}"#)
    );
    assert_eq!(
        view_eval("digest(done.text)").unwrap(),
        s(&digest(&json!({"collection": [["ada", "A"], ["bo", "B"]]}))[..16])
    );
    // A collection of pending results: pending on every instance.
    assert_eq!(
        refs(&view_render("all: ${{ rep.outputs.text }}").unwrap()),
        ["rep['ada']#1", "rep['bo']#1"]
    );
    assert_eq!(
        refs(&view_eval("digest(rep.*.outputs)").unwrap()),
        ["rep['ada']#1", "rep['bo']#1"]
    );
    // accepted() finishes a `.*` result first, and refuses other views.
    assert_eq!(
        view_eval("accepted(done.text)").unwrap(),
        collection(&[("ada", s("A")), ("bo", s("B"))], &[])
    );
    assert_eq!(
        view_eval("accepted(rep)").unwrap_err().0,
        "accepted() needs a collection of a repeated step"
    );
}

#[test]
fn finish_recurses() {
    let mut views = ViewScope::new();
    let every = views.root("done").unwrap();
    let value = Val::List(vec![every, obj(&[("k", num(1.0))])]);
    let finished = finish(&mut views, value).unwrap();
    assert_eq!(
        finished,
        list(&[
            collection(
                &[
                    ("ada", obj(&[("text", s("A"))])),
                    ("bo", obj(&[("text", s("B"))]))
                ],
                &[]
            ),
            obj(&[("k", num(1.0))])
        ])
    );
    let node = views.root("node").unwrap();
    assert_eq!(
        finish(&mut views, obj(&[("n", node)])).unwrap_err(),
        ExprError::names_a_step()
    );
    // MapScope refuses every view.
    let mut map = scope();
    assert_eq!(
        finish(&mut map, Val::View(ViewId(0))).unwrap_err(),
        ExprError::names_a_step()
    );
    assert_eq!(finish(&mut map, ok("x")).unwrap(), ok("x"));
}

#[test]
fn pending_refs_are_sorted_sets() {
    let value = ok("Q + P");
    let Val::Pending(p) = &value else { panic!() };
    assert_eq!(
        p.refs,
        BTreeSet::from(["a#1".to_string(), "b#1".to_string()])
    );
}
