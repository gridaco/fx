//! Routes, route tables and per-call prices (identity.md §7, §12; spec/schemas/fx-routes-v1).
//!
//! A route is `<model>@<provider>`, split at the last `@`. Its fingerprint hashes capability,
//! model, provider and contract (`{}` when none) under `kind: fx-route-v1`; price, features and
//! concurrency are not part of it.
//!
//! The catalog is built from tables in this order (identity.md §7): the built-in default table
//! (left out when any `--routes` file is given), then `fx.yaml` `route_tables` in order, then each
//! `--routes` file in order. The core knows no built-in table of its own: the caller hands it in
//! (the `grida-fx` command passes `grida_fx_providers::routes::default_table()`, the table its
//! adapters serve; tests pass an empty one). A later table's entry for the same capability and route replaces an
//! earlier one, contract included; two entries for the same capability and route within one
//! table are refused (`<route> is declared twice for <capability>`).
//!
//! Prices are read in micro-dollars ([`Usd`]); an amount with more than 6 decimal places is
//! refused (identity.md §12), judged on the number's JCS text (`1e-7 has more than 6 decimal
//! places`). A price's other refusals keep the predecessor's sentences: `a price is a
//! non-negative range, low to high`; `a length-priced route declares the most units one call
//! carries`; `a tiered price names the setting it is priced by, and its tiers`; `price tier
//! {name} lies outside the route's range`. Every refusal reads `<file>: <where>: <sentence>`,
//! `<where>` the instance path joined by `.` as in schema refusals: `routes.0.price.low_usd`,
//! `routes.0.price`, `routes.1` for a duplicate, `routes.0.route` for a bad route id.
//!
//! One call's price for a step's with-values (`Route::cost`): the tier is `with[by]` when it is
//! text, else the whole range. Per call, the range as is. Per unit, the call's length is
//! `len(with.text)` in characters (Unicode scalar values) when it is text, else `with.max_chars`
//! when it is a number (per thousand characters); or the first number of `duration`,
//! `duration_s` (per second). Booleans are not numbers, and a negative number is no length. The
//! high price covers `min(length, max_units)` units, or `max_units` when the length is unknown;
//! the low price covers the known length uncapped, or nothing when it is unknown. Each product is
//! exact and rounds half to even.

use crate::docs::project::Project;
use crate::error::{Error, Result};
use crate::money::{Units, Usd};
use crate::val::Val;
use crate::value::as_f64;
use indexmap::IndexMap;
use serde_json::{Map, Value};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// What a price's amounts are per.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PriceUnit {
    Call,
    Second,
    /// Per thousand characters.
    KChars,
}

/// A route's price (fx-routes-v1 `price`). Read-time refusals are listed in the module doc.
#[derive(Debug, Clone, PartialEq)]
pub struct RoutePrice {
    pub low: Usd,
    pub high: Usd,
    pub unit: PriceUnit,
    pub max_units: Option<Units>,
    pub by: Option<String>,
    pub tiers: IndexMap<String, (Usd, Usd)>,
}

impl RoutePrice {
    /// The low and high price of one call: the tier's range when `tier` names one, else the whole
    /// range; per call as is; per unit, `high = hi × min(known, max_units) / div` and
    /// `low = lo × units / div` when units are known (not capped), else 0.
    pub fn per_call(&self, units: Option<Units>, tier: Option<&str>) -> (Usd, Usd) {
        let (low, high) = tier
            .and_then(|name| self.tiers.get(name))
            .copied()
            .unwrap_or((self.low, self.high));
        let divisor = match self.unit {
            PriceUnit::Call => return (low, high),
            PriceUnit::Second => 1,
            PriceUnit::KChars => 1000,
        };
        // A length-priced route always declares max_units when read from a table.
        let known = match (units, self.max_units) {
            (Some(units), Some(max)) => Some(units.min(max)),
            (Some(units), None) => Some(units),
            (None, max) => max,
        };
        let high = known.map_or(Usd::ZERO, |n| high.times_units(n, divisor));
        let low = units.map_or(Usd::ZERO, |n| low.times_units(n, divisor));
        (low, high)
    }
}

/// One route serving one capability.
#[derive(Debug, Clone, PartialEq)]
pub struct Route {
    pub capability: String,
    pub model: String,
    pub provider: String,
    pub price: RoutePrice,
    pub features: BTreeSet<String>,
    pub concurrency: Option<u32>,
    pub requests_per_minute: Option<u32>,
    /// The declared contract object; `{}` when none.
    pub contract: Value,
}

impl Route {
    /// `model@provider`.
    pub fn id(&self) -> String {
        format!("{}@{}", self.model, self.provider)
    }

    /// The route fingerprint (identity.md §7).
    pub fn fingerprint(&self) -> String {
        let mut object = Map::new();
        object.insert("kind".into(), Value::from("fx-route-v1"));
        object.insert("capability".into(), Value::from(self.capability.as_str()));
        object.insert("model".into(), Value::from(self.model.as_str()));
        object.insert("provider".into(), Value::from(self.provider.as_str()));
        object.insert("contract".into(), self.contract.clone());
        crate::value::digest(&Value::Object(object))
    }

    /// The model behind any provider, lower-cased: `vendor/IMG-A` and `img-a` are the same model.
    /// This is what `independent_of` compares.
    pub fn underlying_model(&self) -> String {
        let name = self.model.rsplit('/').next().unwrap_or(&self.model);
        name.to_lowercase()
    }

    /// The price of one call for an instance's with-values (module doc).
    pub fn cost(&self, with: &IndexMap<String, Val>) -> (Usd, Usd) {
        let tier = self.price.by.as_ref().and_then(|by| match with.get(by) {
            Some(Val::Str(tier)) => Some(tier.as_str()),
            _ => None,
        });
        self.price.per_call(self.units_of(with), tier)
    }

    /// A call's length when the route is priced by length: characters, or seconds.
    fn units_of(&self, with: &IndexMap<String, Val>) -> Option<Units> {
        let number = |name: &str| match with.get(name) {
            Some(Val::Number(x)) if *x >= 0.0 => Some(Some(Units::from_f64(*x))),
            Some(Val::Number(_)) => Some(None),
            _ => None,
        };
        match self.price.unit {
            PriceUnit::Call => None,
            PriceUnit::KChars => match with.get("text") {
                Some(Val::Str(text)) => Some(Units::whole(text.chars().count() as u64)),
                _ => number("max_chars").flatten(),
            },
            PriceUnit::Second => ["duration", "duration_s"]
                .into_iter()
                .find_map(number)
                .flatten(),
        }
    }
}

/// Splits a route id at the last `@`: `a route is model@provider, not {repr(value)}`.
pub fn parse_route_id(value: &str) -> std::result::Result<(String, String), String> {
    match value.rsplit_once('@') {
        Some((model, provider)) if !model.is_empty() && !provider.is_empty() => {
            Ok((model.to_string(), provider.to_string()))
        }
        _ => Err(format!(
            "a route is model@provider, not {}",
            crate::text::py_repr_str(value)
        )),
    }
}

/// Routes by capability and id.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RouteTable {
    pub entries: IndexMap<(String, String), Route>,
}

impl RouteTable {
    pub fn new() -> RouteTable {
        RouteTable::default()
    }

    /// Reads one `fx: routes/v1` table (already loaded from YAML). `file` labels errors; a
    /// refusal is an [`crate::ErrorKind::Route`] error `"<file>: <where>: <message>"` (exit 2)
    /// (module doc), except a document outside the schema, which [`crate::docs::validate`]
    /// refuses.
    pub fn from_document(document: &Value, file: &str) -> Result<RouteTable> {
        crate::docs::validate(crate::docs::Schema::Routes, document, file)?;
        parse_table(document).map_err(|message| Error::route(format!("{file}: {message}")))
    }

    /// Lays a later table over this one: its entries replace equal (capability, route) entries.
    pub fn overlay(&mut self, later: RouteTable) {
        for (key, route) in later.entries {
            self.entries.insert(key, route);
        }
    }

    /// The route `id` serving `capability`: `a route is model@provider, not {repr}` for a bad id;
    /// `no route {id} serves {capability} (known routes: {sorted ids joined ", " or "none"})`.
    pub fn resolve(&self, capability: &str, id: &str) -> std::result::Result<&Route, String> {
        parse_route_id(id)?;
        if let Some(route) = self.entries.get(&(capability.to_string(), id.to_string())) {
            return Ok(route);
        }
        let known: Vec<String> = self.routes_for(capability).iter().map(|r| r.id()).collect();
        let known = if known.is_empty() {
            "none".to_string()
        } else {
            known.join(", ")
        };
        Err(format!(
            "no route {id} serves {capability} (known routes: {known})"
        ))
    }

    /// Every route serving a capability, sorted by id.
    pub fn routes_for(&self, capability: &str) -> Vec<&Route> {
        let mut routes: Vec<(&String, &Route)> = self
            .entries
            .iter()
            .filter(|((cap, _), _)| cap == capability)
            .map(|((_, id), route)| (id, route))
            .collect();
        routes.sort_by(|a, b| a.0.cmp(b.0));
        routes.into_iter().map(|(_, route)| route).collect()
    }
}

/// Builds the catalog for a plan (identity.md §7): `builtin` unless `extra` is non-empty, the
/// project's `route_tables` (relative to its root, labelled by that path), then `extra`:
/// `(absolute path, label as the user typed it)`. Each file is read with the strict YAML loader
/// and validated against fx-routes-v1; a missing file is an Io error naming the label.
pub fn load_catalog(
    project: &Project,
    builtin: &RouteTable,
    extra: &[(PathBuf, String)],
) -> Result<RouteTable> {
    let mut catalog = if extra.is_empty() {
        builtin.clone()
    } else {
        RouteTable::new()
    };
    for table in &project.document.route_tables {
        catalog.overlay(read_table(&project.root.join(table), table)?);
    }
    for (path, label) in extra {
        catalog.overlay(read_table(path, label)?);
    }
    Ok(catalog)
}

/// Reads, checks and parses one route table file.
fn read_table(path: &Path, label: &str) -> Result<RouteTable> {
    let document = crate::docs::read_document(path, label, crate::docs::Schema::Routes)?;
    parse_table(&document).map_err(|message| Error::route(format!("{label}: {message}")))
}

/// The parse step after schema validation: every route of the table, then the duplicate check.
/// Each message starts with where it applies, as the instance path joined by `.`
/// (`routes.0.price.low_usd: …`), as schema refusals do.
fn parse_table(document: &Value) -> std::result::Result<RouteTable, String> {
    let entries = match document.get("routes") {
        Some(Value::Array(entries)) => entries.as_slice(),
        None => &[],
        Some(_) => return Err("routes: not a list".into()),
    };
    let routes = entries
        .iter()
        .enumerate()
        .map(|(i, entry)| parse_route(entry, &format!("routes.{i}")))
        .collect::<std::result::Result<Vec<Route>, String>>()?;
    let mut table = RouteTable::new();
    for (i, route) in routes.into_iter().enumerate() {
        let key = (route.capability.clone(), route.id());
        if table.entries.contains_key(&key) {
            return Err(format!(
                "routes.{i}: {} is declared twice for {}",
                key.1, route.capability
            ));
        }
        table.entries.insert(key, route);
    }
    Ok(table)
}

/// One route entry at `at` (`routes.0`).
fn parse_route(entry: &Value, at: &str) -> std::result::Result<Route, String> {
    let entry = entry
        .as_object()
        .ok_or_else(|| format!("{at}: not an object"))?;
    let text = |key: &str| match entry.get(key) {
        Some(Value::String(s)) => Ok(s.clone()),
        _ => Err(format!("{at}.{key}: missing, or not text")),
    };
    let capability = text("capability")?;
    let (model, provider) =
        parse_route_id(&text("route")?).map_err(|message| format!("{at}.route: {message}"))?;
    let price = parse_price(
        entry
            .get("price")
            .ok_or_else(|| format!("{at}.price: missing"))?,
        &format!("{at}.price"),
    )?;
    let features = match entry.get("features") {
        None => BTreeSet::new(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| {
                item.as_str()
                    .map(str::to_string)
                    .ok_or_else(|| format!("{at}.features: not a list of text"))
            })
            .collect::<std::result::Result<_, _>>()?,
        Some(_) => return Err(format!("{at}.features: not a list of text")),
    };
    let count = |key: &str| match entry.get(key) {
        None => Ok(None),
        Some(Value::Number(n)) => n
            .as_u64()
            .filter(|n| *n >= 1)
            .and_then(|n| u32::try_from(n).ok())
            .map(Some)
            .ok_or_else(|| format!("{at}.{key}: not a whole number of at least 1")),
        Some(_) => Err(format!("{at}.{key}: not a whole number of at least 1")),
    };
    let contract = match entry.get("contract") {
        None => Value::Object(Map::new()),
        Some(contract @ Value::Object(_)) => contract.clone(),
        Some(_) => return Err(format!("{at}.contract: not an object")),
    };
    Ok(Route {
        capability,
        model,
        provider,
        price,
        features,
        concurrency: count("concurrency")?,
        requests_per_minute: count("requests_per_minute")?,
        contract,
    })
}

/// An amount of a price at `at`, negative amounts kept so the range checks can name them.
fn amount(value: &Value, at: &str) -> std::result::Result<Usd, String> {
    let read = match value {
        Value::Number(n) if as_f64(n) < 0.0 => crate::value::number(-as_f64(n))
            .map_err(|refused| refused.message)
            .and_then(|positive| Usd::from_value(&positive))
            .map(|usd| Usd(-usd.0)),
        _ => Usd::from_value(value),
    };
    read.map_err(|message| format!("{at}: {message}"))
}

/// A route's price at `at` (`routes.0.price`).
fn parse_price(price: &Value, at: &str) -> std::result::Result<RoutePrice, String> {
    let price = price
        .as_object()
        .ok_or_else(|| format!("{at}: not an object"))?;
    let read = |key: &str| {
        price
            .get(key)
            .map(|value| amount(value, &format!("{at}.{key}")))
            .transpose()
    };
    let usd = read("usd")?;
    let low = read("low_usd")?.or(usd).unwrap_or(Usd::ZERO);
    let high = read("high_usd")?.or(usd).unwrap_or(Usd::ZERO);
    let unit = match price.get("unit") {
        None => PriceUnit::Call,
        Some(Value::String(unit)) if unit == "call" => PriceUnit::Call,
        Some(Value::String(unit)) if unit == "second" => PriceUnit::Second,
        Some(Value::String(unit)) if unit == "1k_chars" => PriceUnit::KChars,
        Some(_) => return Err(format!("{at}.unit: not call, second or 1k_chars")),
    };
    let max_units = match price.get("max_units") {
        None => None,
        Some(Value::Number(n)) => Some(as_f64(n)),
        Some(_) => return Err(format!("{at}.max_units: not a number")),
    };
    let by = match price.get("by") {
        None => None,
        Some(Value::String(by)) => Some(by.clone()),
        Some(_) => return Err(format!("{at}.by: not text")),
    };
    let mut tiers = IndexMap::new();
    match price.get("tiers") {
        None => {}
        Some(Value::Object(declared)) => {
            for (name, tier) in declared {
                let bound = |key: &str| {
                    let at = format!("{at}.tiers.{name}.{key}");
                    match tier.get(key) {
                        Some(value) => amount(value, &at),
                        None => Err(format!("{at}: missing")),
                    }
                };
                tiers.insert(name.clone(), (bound("low_usd")?, bound("high_usd")?));
            }
        }
        Some(_) => return Err(format!("{at}.tiers: not an object")),
    }
    if low < Usd::ZERO || high < low {
        return Err(format!(
            "{at}: a price is a non-negative range, low to high"
        ));
    }
    if unit != PriceUnit::Call && max_units.is_none_or(|max| max <= 0.0) {
        return Err(format!(
            "{at}: a length-priced route declares the most units one call carries"
        ));
    }
    if by.is_none() != tiers.is_empty() {
        return Err(format!(
            "{at}: a tiered price names the setting it is priced by, and its tiers"
        ));
    }
    for (name, (tier_low, tier_high)) in &tiers {
        if !(low <= *tier_low && tier_low <= tier_high && *tier_high <= high) {
            return Err(format!(
                "{at}: price tier {name} lies outside the route's range"
            ));
        }
    }
    Ok(RoutePrice {
        low,
        high,
        unit,
        max_units: max_units.map(Units::from_f64),
        by,
        tiers,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn table(routes: Value) -> std::result::Result<RouteTable, String> {
        parse_table(&json!({"fx": "routes/v1", "routes": routes}))
    }

    fn price(price: Value) -> std::result::Result<RoutePrice, String> {
        table(json!([{"capability": "c", "route": "m@p", "price": price}]))
            .map(|t| t.entries[0].price.clone())
    }

    fn micros(pair: (Usd, Usd)) -> (i64, i64) {
        (pair.0.0, pair.1.0)
    }

    #[test]
    fn route_ids_split_at_the_last_at() {
        assert_eq!(
            parse_route_id("img-a@acme"),
            Ok(("img-a".into(), "acme".into()))
        );
        assert_eq!(
            parse_route_id("org@x/img@acme"),
            Ok(("org@x/img".into(), "acme".into()))
        );
        for bad in ["img-a", "@acme", "img-a@", "", "@"] {
            assert_eq!(
                parse_route_id(bad).unwrap_err(),
                format!("a route is model@provider, not '{bad}'")
            );
        }
    }

    #[test]
    fn entries_are_read_with_defaults() {
        let read = table(json!([
            {"capability": "image.generate", "route": "vendor/IMG-A@acme", "price": {"usd": 0.04},
             "features": ["mask", "alpha"], "concurrency": 2, "requests_per_minute": 60,
             "contract": {"sizes": ["1024x1024"]}},
            {"capability": "image.generate", "route": "img-b@acme",
             "price": {"low_usd": 0.01, "high_usd": 0.04}},
        ]))
        .unwrap();
        let a = &read.entries[0];
        assert_eq!(a.model, "vendor/IMG-A");
        assert_eq!(a.underlying_model(), "img-a");
        assert_eq!(a.price.low, Usd(40_000));
        assert_eq!(a.price.high, Usd(40_000));
        assert_eq!(a.price.unit, PriceUnit::Call);
        assert_eq!(a.features, BTreeSet::from(["alpha".into(), "mask".into()]));
        assert_eq!(a.concurrency, Some(2));
        assert_eq!(a.requests_per_minute, Some(60));
        assert_eq!(a.contract, json!({"sizes": ["1024x1024"]}));
        let b = &read.entries[1];
        assert_eq!((b.price.low, b.price.high), (Usd(10_000), Usd(40_000)));
        assert_eq!(b.contract, json!({}));
        assert_eq!(b.concurrency, None);
        assert!(b.features.is_empty());
        assert_eq!(price(json!({})).unwrap().high, Usd::ZERO);
        assert_eq!(
            price(json!({"low_usd": 0.5})).unwrap_err(),
            "routes.0.price: a price is a non-negative range, low to high"
        );
        assert!(table(json!([])).unwrap().entries.is_empty());
        assert!(
            parse_table(&json!({"fx": "routes/v1"}))
                .unwrap()
                .entries
                .is_empty()
        );
    }

    #[test]
    fn price_refusals() {
        assert_eq!(
            price(json!({"low_usd": 0.05, "high_usd": 0.04})).unwrap_err(),
            "routes.0.price: a price is a non-negative range, low to high"
        );
        assert_eq!(
            price(json!({"low_usd": -0.01, "high_usd": 0.04})).unwrap_err(),
            "routes.0.price: a price is a non-negative range, low to high"
        );
        assert_eq!(
            price(json!({"usd": 0.1, "unit": "second"})).unwrap_err(),
            "routes.0.price: a length-priced route declares the most units one call carries"
        );
        assert_eq!(
            price(json!({"usd": 0.1, "unit": "1k_chars", "max_units": 0})).unwrap_err(),
            "routes.0.price: a length-priced route declares the most units one call carries"
        );
        assert_eq!(
            price(json!({"usd": 0.1, "by": "resolution"})).unwrap_err(),
            "routes.0.price: a tiered price names the setting it is priced by, and its tiers"
        );
        assert_eq!(
            price(json!({"usd": 0.1, "tiers": {"a": {"low_usd": 0.1, "high_usd": 0.1}}}))
                .unwrap_err(),
            "routes.0.price: a tiered price names the setting it is priced by, and its tiers"
        );
        let tiered = |low: f64, high: f64| {
            price(json!({"low_usd": 0.1, "high_usd": 0.5, "by": "r",
                         "tiers": {"ok": {"low_usd": 0.1, "high_usd": 0.5},
                                   "x": {"low_usd": low, "high_usd": high}}}))
        };
        assert!(tiered(0.2, 0.3).is_ok());
        for (low, high) in [(0.05, 0.3), (0.2, 0.6), (0.3, 0.2), (-0.1, 0.2)] {
            assert_eq!(
                tiered(low, high).unwrap_err(),
                "routes.0.price: price tier x lies outside the route's range",
                "{low} {high}"
            );
        }
        assert_eq!(
            price(json!({"usd": 0.0123456})).unwrap_err(),
            "routes.0.price.usd: 0.0123456 has more than 6 decimal places"
        );
        assert_eq!(
            price(json!({"low_usd": 0.01, "high_usd": 0.0333333})).unwrap_err(),
            "routes.0.price.high_usd: 0.0333333 has more than 6 decimal places"
        );
        assert_eq!(
            price(json!({"low_usd": 0.1, "high_usd": 0.5, "by": "r",
                         "tiers": {"x": {"low_usd": 0.1, "high_usd": 0.0000001}}}))
            .unwrap_err(),
            "routes.0.price.tiers.x.high_usd: 1e-7 has more than 6 decimal places"
        );
        assert!(price(json!({"usd": 0.000001})).is_ok());
    }

    #[test]
    fn duplicates_within_a_table_are_refused() {
        let entry =
            json!({"capability": "image.generate", "route": "img-a@acme", "price": {"usd": 0.04}});
        assert_eq!(
            table(json!([entry, entry])).unwrap_err(),
            "routes.1: img-a@acme is declared twice for image.generate"
        );
        let other =
            json!({"capability": "image.edit", "route": "img-a@acme", "price": {"usd": 0.04}});
        assert_eq!(table(json!([entry, other])).unwrap().entries.len(), 2);
        // Every entry is read before duplicates are looked for.
        let bad = json!({"capability": "x", "route": "nope", "price": {}});
        assert_eq!(
            table(json!([entry, entry, bad])).unwrap_err(),
            "routes.2.route: a route is model@provider, not 'nope'"
        );
    }

    #[test]
    fn shape_errors_name_the_entry() {
        assert_eq!(
            table(json!([{"route": "m@p", "price": {}}])).unwrap_err(),
            "routes.0.capability: missing, or not text"
        );
        assert_eq!(
            table(json!([{"capability": "c", "route": "m@p", "price": {}, "concurrency": 0}]))
                .unwrap_err(),
            "routes.0.concurrency: not a whole number of at least 1"
        );
        assert_eq!(
            table(json!([{"capability": "c", "route": "m@p", "price": {"unit": "minute"}}]))
                .unwrap_err(),
            "routes.0.price.unit: not call, second or 1k_chars"
        );
    }

    #[test]
    fn overlay_replaces_and_resolve_explains() {
        let mut catalog = table(json!([
            {"capability": "image.generate", "route": "img-b@acme", "price": {"usd": 0.02}},
            {"capability": "image.generate", "route": "img-a@acme", "price": {"usd": 0.04}},
            {"capability": "image.edit", "route": "img-a@acme", "price": {"usd": 0.05}},
        ]))
        .unwrap();
        catalog.overlay(
            table(json!([
                {"capability": "image.generate", "route": "img-a@acme", "price": {"usd": 0.01},
                 "contract": {"mask": true}},
                {"capability": "speech.generate", "route": "voice-a@acme", "price": {"usd": 0.3}},
            ]))
            .unwrap(),
        );
        assert_eq!(catalog.entries.len(), 4);
        let replaced = catalog.resolve("image.generate", "img-a@acme").unwrap();
        assert_eq!(replaced.price.high, Usd(10_000));
        assert_eq!(replaced.contract, json!({"mask": true}));
        let ids: Vec<String> = catalog
            .routes_for("image.generate")
            .iter()
            .map(|r| r.id())
            .collect();
        assert_eq!(ids, ["img-a@acme", "img-b@acme"]);
        assert!(catalog.routes_for("mesh.rig").is_empty());
        assert_eq!(
            catalog.resolve("image.generate", "img-c@acme").unwrap_err(),
            "no route img-c@acme serves image.generate (known routes: img-a@acme, img-b@acme)"
        );
        assert_eq!(
            catalog.resolve("mesh.rig", "img-a@acme").unwrap_err(),
            "no route img-a@acme serves mesh.rig (known routes: none)"
        );
        assert_eq!(
            catalog.resolve("image.generate", "img-a").unwrap_err(),
            "a route is model@provider, not 'img-a'"
        );
    }

    fn priced(price: Value) -> Route {
        table(json!([{"capability": "c", "route": "m@p", "price": price}]))
            .unwrap()
            .entries[0]
            .clone()
    }

    fn with(values: &[(&str, Val)]) -> IndexMap<String, Val> {
        values
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    #[test]
    fn per_call_prices() {
        let route = priced(json!({"low_usd": 0.01, "high_usd": 0.04}));
        assert_eq!(micros(route.cost(&with(&[]))), (10_000, 40_000));
        assert_eq!(
            micros(route.price.per_call(Some(Units::whole(5)), None)),
            (10_000, 40_000)
        );
    }

    #[test]
    fn characters_and_seconds() {
        let speech =
            priced(json!({"low_usd": 0.1, "high_usd": 0.3, "unit": "1k_chars", "max_units": 5000}));
        assert_eq!(
            micros(speech.cost(&with(&[("text", Val::Str("h\u{e9}llo w\u{f6}rld".into()))]))),
            (1_100, 3_300)
        );
        let pending = Val::Pending(Box::new(crate::val::Pending {
            refs: BTreeSet::from(["a#1".to_string()]),
            token: "t".into(),
        }));
        assert_eq!(
            micros(speech.cost(&with(&[
                ("text", pending.clone()),
                ("max_chars", Val::Number(1234.0))
            ]))),
            (123_400, 370_200)
        );
        assert_eq!(
            micros(speech.cost(&with(&[("text", pending.clone())]))),
            (0, 1_500_000)
        );
        // Booleans are not numbers; a negative number is no length.
        assert_eq!(
            micros(speech.cost(&with(&[("max_chars", Val::Bool(true))]))),
            (0, 1_500_000)
        );
        assert_eq!(
            micros(speech.cost(&with(&[("max_chars", Val::Number(-5.0))]))),
            (0, 1_500_000)
        );
        // The low price is not capped at max_units.
        assert_eq!(
            micros(speech.cost(&with(&[("max_chars", Val::Number(10_000.0))]))),
            (1_000_000, 1_500_000)
        );
        let clip =
            priced(json!({"low_usd": 0.03, "high_usd": 0.0375, "unit": "second", "max_units": 10}));
        assert_eq!(
            micros(clip.cost(&with(&[("duration", Val::Number(3.0))]))),
            (90_000, 112_500)
        );
        assert_eq!(
            micros(clip.cost(&with(&[("duration_s", Val::Number(2.5))]))),
            (75_000, 93_750)
        );
        assert_eq!(
            micros(clip.cost(&with(&[
                ("duration", Val::Str("3".into())),
                ("duration_s", Val::Number(4.0))
            ]))),
            (120_000, 150_000)
        );
        assert_eq!(
            micros(clip.cost(&with(&[("duration", Val::Bool(true))]))),
            (0, 375_000)
        );
        assert_eq!(micros(clip.cost(&with(&[]))), (0, 375_000));
        assert_eq!(
            micros(clip.cost(&with(&[("duration", Val::Number(20.0))]))),
            (600_000, 375_000)
        );
    }

    #[test]
    fn tiers_follow_the_setting() {
        let video = priced(json!({
            "unit": "second", "max_units": 10, "low_usd": 0.03, "high_usd": 0.375, "by": "resolution",
            "tiers": {"360p": {"low_usd": 0.03, "high_usd": 0.0375}, "4k": {"low_usd": 0.3, "high_usd": 0.375}},
        }));
        let small = video.cost(&with(&[
            ("duration", Val::Number(3.0)),
            ("resolution", Val::Str("360p".into())),
        ]));
        let large = video.cost(&with(&[
            ("duration", Val::Number(8.0)),
            ("resolution", Val::Str("4k".into())),
        ]));
        let unknown = video.cost(&with(&[]));
        assert_eq!(micros(small), (90_000, 112_500));
        assert_eq!(micros(large), (2_400_000, 3_000_000));
        assert_eq!(micros(unknown), (0, 3_750_000));
        assert_eq!(small.0 + large.0 + unknown.0, Usd(2_490_000));
        assert_eq!(small.1 + large.1 + unknown.1, Usd(6_862_500));
        // A tier that is not declared, or a setting that is not text, prices the whole range.
        assert_eq!(
            micros(video.cost(&with(&[
                ("duration", Val::Number(1.0)),
                ("resolution", Val::Str("8k".into()))
            ]))),
            (30_000, 375_000)
        );
        assert_eq!(
            micros(video.cost(&with(&[
                ("duration", Val::Number(1.0)),
                ("resolution", Val::Number(360.0))
            ]))),
            (30_000, 375_000)
        );
    }

    #[test]
    fn fingerprints_ignore_price() {
        let mut route = priced(json!({"usd": 0.04}));
        let before = route.fingerprint();
        route.price.high = Usd(1);
        route.features.insert("mask".into());
        route.concurrency = Some(3);
        assert_eq!(route.fingerprint(), before);
        route.contract = json!({"a": 1});
        assert_ne!(route.fingerprint(), before);
    }
}
