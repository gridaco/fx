//! Routes, route tables and per-call prices (identity.md §7, §12; spec/schemas/fx-routes-v1).
//!
//! A route is `<model>@<provider>`, split at the last `@`. Its fingerprint hashes capability,
//! model, provider and contract (`{}` when none) under `kind: fx-route-v1`; price, features,
//! input-picture limit and concurrency are not part of it.
//!
//! The catalog is built from tables in this order (identity.md §7): the built-in default table
//! (left out when any `--routes` file is given), then `fx.yaml` `route_tables` in order, then each
//! `--routes` file in order. The core knows no built-in route table of its own: the caller hands it
//! in (the `grida-fx` command passes `grida_fx_providers::routes::default_table()`, the table its
//! adapters serve; tests pass an empty one). A later table's entry for the same capability and
//! route replaces an earlier one, contract included; two entries for the same capability and
//! route within one table are refused (`<route> is declared twice for <capability>`).
//!
//! An entry's contract is completed as it is read (identity.md §7), in this order:
//! - `extends` names its base, a route of the same capability and provider: an earlier entry of
//!   the same table, else the entry in force in the catalog so far, else the built-in table's,
//!   which serves as a base even under `--routes` ([`load_catalog`] hands each table these bases;
//!   [`RouteTable::from_document_over`] takes them for a document already loaded). The entry
//!   copies the base's price, features, limits and pacing where it states none, and the base's
//!   contract members it does not state. Refused: `<base> is not a <provider> route`; `no route
//!   <base> serves <capability>`.
//! - [`BUILT_IN_ADAPTERS`]: on a capability and provider FX's adapters serve, a left-out or `null`
//!   `adapter` becomes the name of that pair's default, and a left-out or `null`
//!   `adapter_behavior` the behaviour of the pair's adapter `adapter` names ([`complete_contract`]).
//!   On a provider the planning project declares with a `family`, [`FAMILY_ADAPTERS`] does the
//!   same for the capabilities that family serves: the family is read from the declarations
//!   ([`load_catalog`] passes the project's), never from the provider's name. The core knows FX's
//!   adapters by these tables alone; the providers crate's tests hold them to every adapter it
//!   registers.
//!
//! Prices are read in micro-dollars ([`Usd`]); an amount with more than 6 decimal places is
//! refused (identity.md §12), judged on the number's JCS text (`1e-7 has more than 6 decimal
//! places`). A capped route's price is `cap_usd` alone: per call, $0 to the cap, and
//! [`RoutePrice::capped`]. A price's other refusals read: `a price states an amount (usd,
//! low_usd, high_usd) or a cap (cap_usd)`; `a cap (cap_usd) stands alone`; `a cap is more than
//! 0` (at `.cap_usd`); `a price is a non-negative range, low to high`; `a length-priced route
//! declares the most units one call carries`; `a tiered price names the setting it is priced by,
//! and its tiers`; `price tier {name} lies outside the route's range`. The schema refuses a cap
//! beside other members, or of 0, before these are read. Every refusal reads `<file>: <where>:
//! <sentence>`, `<where>` the instance path joined by `.` as in schema refusals:
//! `routes.0.price.low_usd`, `routes.0.price`, `routes.1` for a duplicate, `routes.0.route` for a
//! bad route id, `routes.0.extends` for a base.
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
use crate::providers::Declarations;
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
    /// A capped route (`cap_usd`): no price is known, so `low` is 0 and `high` the cap, per call
    /// and untiered. Each attempt holds the cap and is charged what the provider reports, else the
    /// cap (spec/adapters.md §5).
    pub capped: bool,
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
    /// The most input pictures one call takes (spec/capabilities.md §1); `None`: no limit is
    /// declared.
    pub max_input_pictures: Option<u32>,
    pub concurrency: Option<u32>,
    pub requests_per_minute: Option<u32>,
    /// The contract object, completed as the module doc says; `{}` when none.
    pub contract: Value,
}

/// The contract one of FX's adapters serves (spec/providers.md §2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuiltInAdapter {
    pub capability: &'static str,
    pub provider: &'static str,
    /// The contract's `adapter`.
    pub adapter: &'static str,
    /// The contract's `adapter_behavior` as canonical JSON: `"1"` (the string) or `1` (the
    /// number).
    pub behavior: &'static str,
    /// Whether it is its capability and provider's default, which serves a route whose contract
    /// names no `adapter`: one per pair. Any other serves only a route that names it.
    pub default: bool,
}

impl BuiltInAdapter {
    /// The behaviour as a JSON value.
    pub fn behavior_value(&self) -> Value {
        serde_json::from_str(self.behavior).expect("a built-in behaviour is JSON")
    }
}

const fn served(
    capability: &'static str,
    provider: &'static str,
    adapter: &'static str,
    behavior: &'static str,
) -> BuiltInAdapter {
    BuiltInAdapter {
        capability,
        provider,
        adapter,
        behavior,
        default: true,
    }
}

/// An adapter that serves only a route whose contract names it.
const fn named(
    capability: &'static str,
    provider: &'static str,
    adapter: &'static str,
    behavior: &'static str,
) -> BuiltInAdapter {
    BuiltInAdapter {
        default: false,
        ..served(capability, provider, adapter, behavior)
    }
}

/// The adapters FX ships, as spec/providers.md §2 lists them: each capability and provider's
/// default, in its first table's order, then the adapters a route names, in its second's.
pub const BUILT_IN_ADAPTERS: [BuiltInAdapter; 30] = [
    served("image.generate", "openai", "fx-openai-image-v1", r#""1""#),
    served("image.edit", "openai", "fx-openai-image-v1", r#""1""#),
    served("image.generate", "openrouter", "openrouter-image", r#""1""#),
    served("image.edit", "openrouter", "openrouter-image", r#""1""#),
    served(
        "structured.generate",
        "openrouter",
        "openrouter-structured",
        "1",
    ),
    served("agent.turn", "openrouter", "openrouter-tool-loop", "1"),
    served("music.generate", "openrouter", "openrouter-music", "1"),
    served("image.generate", "fal", "fal-image", r#""1""#),
    served("image.edit", "fal", "fal-image", r#""1""#),
    served("video.generate", "fal", "fal-queue", "1"),
    served("background.remove", "fal", "fal-run-birefnet", "1"),
    served("mesh.generate", "tripo", "tripo-multiview", "1"),
    served("mesh.rig", "tripo", "tripo-rig", "1"),
    served(
        "sound.generate",
        "elevenlabs",
        "elevenlabs-sound-effect",
        "1",
    ),
    served("speech.generate", "elevenlabs", "elevenlabs-speech", "1"),
    served("image.generate", "codex", "fx-codex-image-v1", r#""2""#),
    served("image.edit", "codex", "fx-codex-image-v1", r#""2""#),
    served("image.generate", "gg", "gg-image", r#""1""#),
    served("image.edit", "gg", "gg-image", r#""1""#),
    served("structured.generate", "gg", "gg-structured", "1"),
    served("agent.turn", "gg", "gg-tool-loop", "1"),
    served("video.generate", "gg", "gg-video", "1"),
    served("mesh.generate", "gg", "gg-mesh", "1"),
    served("mesh.rig", "gg", "gg-rig", "1"),
    served("image.generate", "grok", "fx-grok-image-v1", r#""1""#),
    served("image.edit", "grok", "fx-grok-image-v1", r#""1""#),
    named(
        "image.generate",
        "openrouter",
        "fx-openrouter-image-v1",
        r#""3""#,
    ),
    named(
        "image.edit",
        "openrouter",
        "fx-openrouter-image-v1",
        r#""3""#,
    ),
    named("image.generate", "fal", "fx-fal-image-v1", r#""1""#),
    named("image.edit", "fal", "fx-fal-image-v1", r#""1""#),
];

/// The contract one of a family's adapters serves (spec/providers.md §2): the default of its
/// capability on every provider the planning project declares with that family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FamilyAdapter {
    /// The declaration's `family`.
    pub family: &'static str,
    pub capability: &'static str,
    /// The contract's `adapter`.
    pub adapter: &'static str,
    /// The contract's `adapter_behavior` as canonical JSON.
    pub behavior: &'static str,
}

/// The adapters of each family a declaration may name, as spec/providers.md §2's third table
/// lists them.
pub const FAMILY_ADAPTERS: [FamilyAdapter; 2] = [
    FamilyAdapter {
        family: "openai-compatible",
        capability: "structured.generate",
        adapter: "openai-compatible-structured",
        behavior: "1",
    },
    FamilyAdapter {
        family: "openai-compatible",
        capability: "agent.turn",
        adapter: "openai-compatible-tool-loop",
        behavior: "1",
    },
];

/// The adapter of `family` that serves `capability`, if any.
pub fn family_adapter(family: &str, capability: &str) -> Option<&'static FamilyAdapter> {
    FAMILY_ADAPTERS
        .iter()
        .find(|served| served.family == family && served.capability == capability)
}

/// The default FX ships for `capability` on `provider`, if any: the adapter of a route whose
/// contract names none.
pub fn built_in_adapter(capability: &str, provider: &str) -> Option<&'static BuiltInAdapter> {
    BUILT_IN_ADAPTERS.iter().find(|served| {
        served.default && served.capability == capability && served.provider == provider
    })
}

/// The adapter FX ships for `capability` on `provider` under the contract `adapter` name `adapter`,
/// default or not.
pub fn built_in_adapter_named(
    capability: &str,
    provider: &str,
    adapter: &str,
) -> Option<&'static BuiltInAdapter> {
    BUILT_IN_ADAPTERS.iter().find(|served| {
        served.capability == capability && served.provider == provider && served.adapter == adapter
    })
}

/// The adapters that serve `capability` on `provider`, as (contract `adapter`, behaviour as
/// canonical JSON, whether it is the default): FX's own, else those of the family `declarations`
/// give the provider.
fn serving(
    capability: &str,
    provider: &str,
    declarations: &Declarations,
) -> Vec<(&'static str, &'static str, bool)> {
    let own: Vec<_> = BUILT_IN_ADAPTERS
        .iter()
        .filter(|served| served.capability == capability && served.provider == provider)
        .map(|served| (served.adapter, served.behavior, served.default))
        .collect();
    if !own.is_empty() {
        return own;
    }
    let family = declarations
        .get(provider)
        .and_then(|declaration| declaration.family.as_deref());
    family
        .and_then(|family| family_adapter(family, capability))
        .map(|served| (served.adapter, served.behavior, true))
        .into_iter()
        .collect()
}

/// Completes `contract` from the adapters that serve `capability` on `provider`, if any
/// (identity.md §7 step 2): FX's own, else those of the family `declarations` give `provider`. A
/// left-out or `null` `adapter` becomes the default's name, then a left-out or `null`
/// `adapter_behavior` the behaviour of the adapter `adapter` names.
pub fn complete_contract(
    capability: &str,
    provider: &str,
    declarations: &Declarations,
    contract: &mut Map<String, Value>,
) {
    let adapters = serving(capability, provider, declarations);
    let Some(&(default, _, _)) = adapters.iter().find(|(_, _, default)| *default) else {
        return;
    };
    let left_out = |member: Option<&Value>| member.is_none_or(Value::is_null);
    if left_out(contract.get("adapter")) {
        contract.insert("adapter".into(), Value::from(default));
    }
    let named = contract.get("adapter").and_then(Value::as_str);
    let behavior = named
        .and_then(|name| adapters.iter().find(|(adapter, _, _)| *adapter == name))
        .map(|(_, behavior, _)| *behavior);
    if let Some(behavior) = behavior
        && left_out(contract.get("adapter_behavior"))
    {
        let value = serde_json::from_str(behavior).expect("a behaviour is JSON");
        contract.insert("adapter_behavior".into(), value);
    }
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

    /// Reads one `fx: routes/v1` table (already loaded from YAML) on its own, with FX's own
    /// declarations: an entry's `extends` names an earlier entry of the same table. `file` labels
    /// errors; a refusal is an [`crate::ErrorKind::Route`] error `"<file>: <where>: <message>"`
    /// (exit 2) (module doc), except a document outside the schema, which
    /// [`crate::docs::validate`] refuses.
    pub fn from_document(document: &Value, file: &str) -> Result<RouteTable> {
        RouteTable::from_document_over(document, file, &[], &Declarations::built_in())
    }

    /// [`RouteTable::from_document`] for a table read after others, for a planning project whose
    /// declarations are `declarations`: an entry's `extends` names an earlier entry of the same
    /// table, else an entry of the first of `bases` that holds one (the catalog so far, then the
    /// built-in table: identity.md §7), and a declared family completes its provider's contracts.
    pub fn from_document_over(
        document: &Value,
        file: &str,
        bases: &[&RouteTable],
        declarations: &Declarations,
    ) -> Result<RouteTable> {
        crate::docs::validate(crate::docs::Schema::Routes, document, file)?;
        parse_table(document, bases, declarations)
            .map_err(|message| Error::route(format!("{file}: {message}")))
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
/// and validated against fx-routes-v1; a missing file is an Io error naming the label. Each
/// table's `extends` resolves against the catalog so far, then `builtin`, even when `extra`
/// leaves it out of the catalog. The project's declarations complete the contracts of the
/// providers it declares with a family.
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
    let listed = project
        .document
        .route_tables
        .iter()
        .map(|table| (project.root.join(table), table.clone()));
    let declarations = project.declarations();
    for (path, label) in listed.chain(extra.iter().cloned()) {
        let table = read_table(&path, &label, &[&catalog, builtin], &declarations)?;
        catalog.overlay(table);
    }
    Ok(catalog)
}

/// Reads, checks and parses one route table file over `bases`.
fn read_table(
    path: &Path,
    label: &str,
    bases: &[&RouteTable],
    declarations: &Declarations,
) -> Result<RouteTable> {
    let document = crate::docs::read_document(path, label, crate::docs::Schema::Routes)?;
    parse_table(&document, bases, declarations)
        .map_err(|message| Error::route(format!("{label}: {message}")))
}

/// The parse step after schema validation: every route of the table in order, each `extends`
/// resolved against the entries before it, then `bases`; then the duplicate check. Each message
/// starts with where it applies, as the instance path joined by `.` (`routes.0.price.low_usd: …`),
/// as schema refusals do. `declarations` give the families that complete a declared provider's
/// contracts.
fn parse_table(
    document: &Value,
    bases: &[&RouteTable],
    declarations: &Declarations,
) -> std::result::Result<RouteTable, String> {
    let entries = match document.get("routes") {
        Some(Value::Array(entries)) => entries.as_slice(),
        None => &[],
        Some(_) => return Err("routes: not a list".into()),
    };
    let mut routes: Vec<Route> = Vec::with_capacity(entries.len());
    for (i, entry) in entries.iter().enumerate() {
        let base = |capability: &str, id: &str| {
            let key = (capability.to_string(), id.to_string());
            routes
                .iter()
                .rev()
                .find(|route| route.capability == capability && route.id() == id)
                .or_else(|| bases.iter().find_map(|table| table.entries.get(&key)))
                .cloned()
        };
        let route = parse_route(entry, &format!("routes.{i}"), base, declarations)?;
        routes.push(route);
    }
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

/// One route entry at `at` (`routes.0`); `base_of(capability, id)` is the route `extends` may
/// name (module doc).
fn parse_route(
    entry: &Value,
    at: &str,
    base_of: impl Fn(&str, &str) -> Option<Route>,
    declarations: &Declarations,
) -> std::result::Result<Route, String> {
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
    let base = match entry.get("extends") {
        None => None,
        Some(_) => {
            let id = text("extends")?;
            let (_, of) =
                parse_route_id(&id).map_err(|message| format!("{at}.extends: {message}"))?;
            if of != provider {
                return Err(format!("{at}.extends: {id} is not a {provider} route"));
            }
            let base = base_of(&capability, &id)
                .ok_or_else(|| format!("{at}.extends: no route {id} serves {capability}"))?;
            Some(base)
        }
    };
    let price = match (entry.get("price"), &base) {
        (Some(price), _) => parse_price(price, &format!("{at}.price"))?,
        (None, Some(base)) => base.price.clone(),
        (None, None) => return Err(format!("{at}.price: missing")),
    };
    let features = match entry.get("features") {
        None => base
            .as_ref()
            .map(|base| base.features.clone())
            .unwrap_or_default(),
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
    let count = |key: &str, inherited: Option<u32>| match entry.get(key) {
        None => Ok(inherited),
        Some(Value::Number(n)) => n
            .as_u64()
            .filter(|n| *n >= 1)
            .and_then(|n| u32::try_from(n).ok())
            .map(Some)
            .ok_or_else(|| format!("{at}.{key}: not a whole number of at least 1")),
        Some(_) => Err(format!("{at}.{key}: not a whole number of at least 1")),
    };
    let inherited = |limit: fn(&Route) -> Option<u32>| base.as_ref().and_then(limit);
    let max_input_pictures = count(
        "max_input_pictures",
        inherited(|base| base.max_input_pictures),
    )?;
    let concurrency = count("concurrency", inherited(|base| base.concurrency))?;
    let requests_per_minute = count(
        "requests_per_minute",
        inherited(|base| base.requests_per_minute),
    )?;
    let mut contract = match entry.get("contract") {
        None => Map::new(),
        Some(Value::Object(contract)) => contract.clone(),
        Some(_) => return Err(format!("{at}.contract: not an object")),
    };
    // The base's contract members the entry does not state: a merge one level deep.
    if let Some(Value::Object(members)) = base.as_ref().map(|base| &base.contract) {
        for (member, value) in members {
            if !contract.contains_key(member) {
                contract.insert(member.clone(), value.clone());
            }
        }
    }
    complete_contract(&capability, &provider, declarations, &mut contract);
    Ok(Route {
        capability,
        model,
        provider,
        price,
        features,
        max_input_pictures,
        concurrency,
        requests_per_minute,
        contract: Value::Object(contract),
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
    let cap = read("cap_usd")?;
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
    if let Some(cap) = cap {
        if price.len() > 1 {
            return Err(format!("{at}: a cap (cap_usd) stands alone"));
        }
        if cap <= Usd::ZERO {
            return Err(format!("{at}.cap_usd: a cap is more than 0"));
        }
        return Ok(RoutePrice {
            low: Usd::ZERO,
            high: cap,
            unit: PriceUnit::Call,
            max_units: None,
            by: None,
            tiers: IndexMap::new(),
            capped: true,
        });
    }
    // An empty price was $0 up to 0.6.1; a route with no known price is capped instead.
    if !["usd", "low_usd", "high_usd"]
        .iter()
        .any(|member| price.contains_key(*member))
    {
        return Err(format!(
            "{at}: a price states an amount (usd, low_usd, high_usd) or a cap (cap_usd)"
        ));
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
        capped: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn table(routes: Value) -> std::result::Result<RouteTable, String> {
        parse_table(
            &json!({"fx": "routes/v1", "routes": routes}),
            &[],
            &Declarations::built_in(),
        )
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
             "max_input_pictures": 5,
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
        assert_eq!(a.max_input_pictures, Some(5));
        assert_eq!(a.contract, json!({"sizes": ["1024x1024"]}));
        let b = &read.entries[1];
        assert_eq!((b.price.low, b.price.high), (Usd(10_000), Usd(40_000)));
        assert_eq!(b.contract, json!({}));
        assert_eq!(b.concurrency, None);
        assert!(b.features.is_empty());
        assert!(!a.price.capped && !b.price.capped);
        assert_eq!(price(json!({"high_usd": 0.5})).unwrap().low, Usd::ZERO);
        assert_eq!(
            price(json!({"low_usd": 0.5})).unwrap_err(),
            "routes.0.price: a price is a non-negative range, low to high"
        );
        assert!(table(json!([])).unwrap().entries.is_empty());
        assert!(
            parse_table(&json!({"fx": "routes/v1"}), &[], &Declarations::built_in())
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
        // A price that states no amount is no longer $0.
        for empty in [json!({}), json!({"unit": "second", "max_units": 10})] {
            assert_eq!(
                price(empty).unwrap_err(),
                "routes.0.price: a price states an amount (usd, low_usd, high_usd) or a cap \
                 (cap_usd)"
            );
        }
    }

    #[test]
    fn a_cap_prices_each_call_at_nothing_to_the_cap() {
        let capped = priced(json!({"cap_usd": 1.5}));
        assert_eq!(
            capped.price,
            RoutePrice {
                low: Usd::ZERO,
                high: Usd(1_500_000),
                unit: PriceUnit::Call,
                max_units: None,
                by: None,
                tiers: IndexMap::new(),
                capped: true,
            }
        );
        assert_eq!(
            micros(capped.cost(&with(&[("duration", Val::Number(5.0))]))),
            (0, 1_500_000)
        );
        assert!(price(json!({"cap_usd": 0.000001})).unwrap().capped);
        for (beside, member) in [("usd", 0.1), ("low_usd", 0.1), ("high_usd", 2.0)] {
            assert_eq!(
                price(json!({"cap_usd": 1.5, beside: member})).unwrap_err(),
                "routes.0.price: a cap (cap_usd) stands alone"
            );
        }
        assert_eq!(
            price(json!({"cap_usd": 1.5, "unit": "second", "max_units": 10})).unwrap_err(),
            "routes.0.price: a cap (cap_usd) stands alone"
        );
        for nothing in [json!(0), json!(-1)] {
            assert_eq!(
                price(json!({"cap_usd": nothing})).unwrap_err(),
                "routes.0.price.cap_usd: a cap is more than 0"
            );
        }
        assert_eq!(
            price(json!({"cap_usd": 0.0000001})).unwrap_err(),
            "routes.0.price.cap_usd: 1e-7 has more than 6 decimal places"
        );
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
        let bad = json!({"capability": "x", "route": "nope", "price": {"usd": 0.04}});
        assert_eq!(
            table(json!([entry, entry, bad])).unwrap_err(),
            "routes.2.route: a route is model@provider, not 'nope'"
        );
    }

    #[test]
    fn shape_errors_name_the_entry() {
        assert_eq!(
            table(json!([{"route": "m@p", "price": {"usd": 0.04}}])).unwrap_err(),
            "routes.0.capability: missing, or not text"
        );
        let paced = json!([{"capability": "c", "route": "m@p", "price": {"usd": 0.04},
                            "concurrency": 0}]);
        assert_eq!(
            table(paced).unwrap_err(),
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

    #[test]
    fn built_in_adapters_have_one_default_per_capability_and_provider() {
        let defaults: Vec<(&str, &str)> = BUILT_IN_ADAPTERS
            .iter()
            .filter(|served| served.default)
            .map(|served| (served.capability, served.provider))
            .collect();
        let pairs: BTreeSet<(&str, &str)> = BUILT_IN_ADAPTERS
            .iter()
            .map(|served| (served.capability, served.provider))
            .collect();
        assert_eq!(defaults.len(), pairs.len());
        assert_eq!(defaults.iter().copied().collect::<BTreeSet<_>>(), pairs);
        let named: BTreeSet<(&str, &str, &str)> = BUILT_IN_ADAPTERS
            .iter()
            .map(|served| (served.capability, served.provider, served.adapter))
            .collect();
        assert_eq!(named.len(), BUILT_IN_ADAPTERS.len());
        for served in BUILT_IN_ADAPTERS {
            let behavior = served.behavior_value();
            assert_eq!(crate::value::canon(&behavior), served.behavior);
            // Images carry their behaviour as text, every other capability as the number 1.
            assert_eq!(
                behavior.is_string(),
                served.capability.starts_with("image."),
                "{served:?}"
            );
        }
        assert_eq!(
            built_in_adapter("image.edit", "gg").map(|served| served.adapter),
            Some("gg-image")
        );
        assert_eq!(
            built_in_adapter("image.edit", "openrouter").map(|served| served.adapter),
            Some("openrouter-image")
        );
        assert_eq!(
            built_in_adapter_named("image.edit", "openrouter", "fx-openrouter-image-v1")
                .map(|served| (served.behavior, served.default)),
            Some((r#""3""#, false))
        );
        assert_eq!(
            built_in_adapter_named("image.edit", "fal", "fx-openrouter-image-v1"),
            None
        );
        assert_eq!(built_in_adapter("vision.review", "gg"), None);
        assert_eq!(built_in_adapter("image.generate", "acme"), None);
    }

    fn contract_of(capability: &str, route: &str, contract: Value) -> Value {
        let entry = json!({"capability": capability, "route": route, "price": {"usd": 0.01},
                           "contract": contract});
        table(json!([entry])).unwrap().entries[0].contract.clone()
    }

    #[test]
    fn a_contract_is_completed_from_the_adapter_that_serves_its_pair() {
        let image = |contract| contract_of("image.generate", "img-a@gg", contract);
        let filled = json!({"adapter": "gg-image", "adapter_behavior": "1"});
        assert_eq!(image(json!({})), filled);
        assert_eq!(
            image(json!({"adapter": null, "adapter_behavior": null})),
            filled
        );
        assert_eq!(
            image(json!({"quality": "max"})),
            json!({"adapter": "gg-image", "adapter_behavior": "1", "quality": "max"})
        );
        // A stated behaviour is kept, and refused when a call is sent (spec/adapters.md §4).
        assert_eq!(
            image(json!({"adapter_behavior": 1})),
            json!({"adapter": "gg-image", "adapter_behavior": 1})
        );
        // Another adapter gets no behaviour.
        assert_eq!(
            image(json!({"adapter": "acme-image"})),
            json!({"adapter": "acme-image"})
        );
        // Every capability but the images takes the number 1.
        assert_eq!(
            contract_of(
                "speech.generate",
                "voice-a@elevenlabs",
                json!({"adapter": "elevenlabs-speech"})
            ),
            json!({"adapter": "elevenlabs-speech", "adapter_behavior": 1})
        );
        // A pair with more than one adapter fills in its default, and the behaviour of the one
        // the contract names.
        let open = |contract| contract_of("image.edit", "acme/img-a@openrouter", contract);
        assert_eq!(
            open(json!({})),
            json!({"adapter": "openrouter-image", "adapter_behavior": "1"})
        );
        assert_eq!(
            open(json!({"adapter": "fx-openrouter-image-v1"})),
            json!({"adapter": "fx-openrouter-image-v1", "adapter_behavior": "3"})
        );
        // GPT Image's behaviour alone names the default, which refuses it when a call is sent;
        // up to 0.6.1 it was served by GPT Image's adapter.
        assert_eq!(
            open(json!({"adapter_behavior": "3"})),
            json!({"adapter": "openrouter-image", "adapter_behavior": "3"})
        );
        assert_eq!(
            contract_of(
                "image.generate",
                "acme/img-a@fal",
                json!({"adapter": "fx-fal-image-v1", "adapter_behavior": null})
            ),
            json!({"adapter": "fx-fal-image-v1", "adapter_behavior": "1"})
        );
        assert_eq!(
            open(json!({"adapter": "fal-image"})),
            json!({"adapter": "fal-image"}),
            "another pair's adapter gets no behaviour"
        );
        // A pair no adapter of FX serves keeps its contract, on any provider.
        assert_eq!(
            contract_of("vision.review", "review-a@gg", json!({})),
            json!({})
        );
        assert_eq!(
            contract_of("image.generate", "img-a@acme", json!({"adapter": null})),
            json!({"adapter": null})
        );
        // A table without a contract is completed too.
        let bare = table(json!([{"capability": "mesh.rig", "route": "rig-a@tripo",
                                 "price": {"usd": 0.3}}]))
        .unwrap();
        assert_eq!(
            bare.entries[0].contract,
            json!({"adapter": "tripo-rig", "adapter_behavior": 1})
        );
    }

    /// FX's declarations, then `acme` declared with `family` (or none) and `bare` without one.
    fn declaring(family: Option<&str>) -> Declarations {
        let declared = |name: &str, family: Option<&str>| crate::providers::Declaration {
            name: name.into(),
            label: name.into(),
            credential: None,
            base_url: crate::providers::BaseUrl {
                variable: None,
                default: format!("https://api.{name}.test/v1"),
            },
            download_hosts: Vec::new(),
            family: family.map(String::from),
        };
        Declarations::with(&[declared("acme", family), declared("bare", None)])
    }

    #[test]
    fn a_declared_family_completes_its_provider_s_contracts() {
        let read = |declarations: &Declarations, capability: &str, route: &str, contract| {
            let entry = json!({"capability": capability, "route": route,
                               "price": {"cap_usd": 0.05}, "contract": contract});
            let document = json!({"fx": "routes/v1", "routes": [entry]});
            parse_table(&document, &[], declarations).unwrap().entries[0]
                .contract
                .clone()
        };
        let family = declaring(Some("openai-compatible"));
        assert_eq!(
            read(&family, "structured.generate", "text-a@acme", json!({})),
            json!({"adapter": "openai-compatible-structured", "adapter_behavior": 1})
        );
        assert_eq!(
            read(
                &family,
                "agent.turn",
                "text-a@acme",
                json!({"adapter": null})
            ),
            json!({"adapter": "openai-compatible-tool-loop", "adapter_behavior": 1})
        );
        // A stated member is kept; another adapter gets no behaviour.
        assert_eq!(
            read(
                &family,
                "structured.generate",
                "text-a@acme",
                json!({"adapter_behavior": 2})
            ),
            json!({"adapter": "openai-compatible-structured", "adapter_behavior": 2})
        );
        assert_eq!(
            read(
                &family,
                "structured.generate",
                "text-a@acme",
                json!({"adapter": "acme-chat"})
            ),
            json!({"adapter": "acme-chat"})
        );
        // A capability the family does not serve, a provider declared without a family, and one
        // not declared keep their contracts, whatever their names.
        for (declarations, capability, route) in [
            (&family, "image.generate", "img-a@acme"),
            (&family, "structured.generate", "text-a@bare"),
            (&family, "structured.generate", "text-a@other"),
            (&declaring(None), "structured.generate", "text-a@acme"),
            (
                &Declarations::built_in(),
                "structured.generate",
                "text-a@acme",
            ),
        ] {
            assert_eq!(
                read(declarations, capability, route, json!({})),
                json!({}),
                "{capability} {route}"
            );
        }
        // FX's own providers are completed from FX's adapters, whatever is declared.
        assert_eq!(
            read(&family, "structured.generate", "text-a@gg", json!({})),
            json!({"adapter": "gg-structured", "adapter_behavior": 1})
        );
        for served in FAMILY_ADAPTERS {
            assert_eq!(served.behavior, "1", "{served:?}");
            assert_eq!(
                family_adapter(served.family, served.capability),
                Some(&served)
            );
        }
        assert_eq!(family_adapter("openai-compatible", "image.generate"), None);
    }

    #[test]
    fn extends_copies_its_base_and_merges_its_contract() {
        let builtin = table(json!([
            {"capability": "image.generate", "route": "img-base@gg",
             "price": {"low_usd": 0.1, "high_usd": 0.5, "by": "size",
                       "tiers": {"1024x1024": {"low_usd": 0.2, "high_usd": 0.3}}},
             "features": ["alpha", "text_to_image"], "max_input_pictures": 4, "concurrency": 1,
             "requests_per_minute": 60,
             "contract": {"adapter": "gg-image", "adapter_behavior": "1", "quality": "max",
                          "nested": {"a": 1}}},
        ]))
        .unwrap();
        let read = |routes: Value| {
            parse_table(
                &json!({"fx": "routes/v1", "routes": routes}),
                &[&builtin],
                &Declarations::built_in(),
            )
        };
        let extended = read(json!([
            {"capability": "image.generate", "route": "img-a@gg", "extends": "img-base@gg"},
        ]))
        .unwrap();
        let base = &builtin.entries[0];
        let route = &extended.entries[0];
        assert_eq!(route.id(), "img-a@gg");
        assert_eq!(route.price, base.price);
        assert_eq!(route.features, base.features);
        assert_eq!(
            (
                route.max_input_pictures,
                route.concurrency,
                route.requests_per_minute
            ),
            (Some(4), Some(1), Some(60))
        );
        assert_eq!(route.contract, base.contract);
        // A stated member replaces the base's whole; the contract merges one level deep.
        let restated = read(json!([
            {"capability": "image.generate", "route": "img-a@gg", "extends": "img-base@gg",
             "price": {"usd": 0.4}, "features": ["text_to_image"], "concurrency": 2,
             "contract": {"quality": "high", "nested": {"b": 2}}},
        ]))
        .unwrap();
        let route = &restated.entries[0];
        assert_eq!(
            (route.price.low, route.price.high),
            (Usd(400_000), Usd(400_000))
        );
        assert!(route.price.tiers.is_empty());
        assert_eq!(route.features, BTreeSet::from(["text_to_image".into()]));
        assert_eq!(
            (
                route.max_input_pictures,
                route.concurrency,
                route.requests_per_minute
            ),
            (Some(4), Some(2), Some(60))
        );
        assert_eq!(
            route.contract,
            json!({"adapter": "gg-image", "adapter_behavior": "1", "quality": "high",
                   "nested": {"b": 2}})
        );
        // An earlier entry of the same table wins over a base of the same route, and an entry
        // may extend its own route.
        let chained = read(json!([
            {"capability": "image.generate", "route": "img-base@gg", "extends": "img-base@gg",
             "contract": {"quality": "low"}},
            {"capability": "image.generate", "route": "img-a@gg", "extends": "img-base@gg"},
        ]))
        .unwrap();
        assert_eq!(chained.entries[0].price, base.price);
        assert_eq!(chained.entries[1].contract["quality"], json!("low"));
        // A restated cap replaces the base's whole price, tiers included, and a cap is inherited
        // as any price is.
        let capped = read(json!([
            {"capability": "image.generate", "route": "img-a@gg", "extends": "img-base@gg",
             "price": {"cap_usd": 2}},
            {"capability": "image.generate", "route": "img-b@gg", "extends": "img-a@gg"},
        ]))
        .unwrap();
        for route in capped.entries.values() {
            assert!(route.price.capped, "{}", route.id());
            assert_eq!(
                (route.price.low, route.price.high),
                (Usd::ZERO, Usd(2_000_000))
            );
            assert!(route.price.tiers.is_empty() && route.price.by.is_none());
        }
        // Without a base, an entry names its price.
        assert_eq!(
            table(json!([{"capability": "image.generate", "route": "img-a@gg"}])).unwrap_err(),
            "routes.0.price: missing"
        );
        // `extends` is not part of the fingerprint: stating the same members is the same route.
        let stated = read(json!([
            {"capability": "image.generate", "route": "img-a@gg", "price": {"usd": 0.1},
             "contract": {"quality": "max", "nested": {"a": 1}}},
        ]))
        .unwrap();
        assert_eq!(
            stated.entries[0].fingerprint(),
            extended.entries[0].fingerprint()
        );
    }

    #[test]
    fn extends_refusals() {
        let builtin = table(json!([
            {"capability": "image.generate", "route": "img-base@gg", "price": {"usd": 0.1}},
        ]))
        .unwrap();
        let read = |extends: &str, capability: &str| {
            let routes = json!([{"capability": capability, "route": "img-a@gg",
                                 "extends": extends}]);
            parse_table(
                &json!({"fx": "routes/v1", "routes": routes}),
                &[&builtin],
                &Declarations::built_in(),
            )
            .unwrap_err()
        };
        assert_eq!(
            read("img-z@gg", "image.generate"),
            "routes.0.extends: no route img-z@gg serves image.generate"
        );
        assert_eq!(
            read("img-base@gg", "image.edit"),
            "routes.0.extends: no route img-base@gg serves image.edit"
        );
        assert_eq!(
            read("img-base@acme", "image.generate"),
            "routes.0.extends: img-base@acme is not a gg route"
        );
        assert_eq!(
            read("img-base", "image.generate"),
            "routes.0.extends: a route is model@provider, not 'img-base'"
        );
        // A later entry of the same table is no base yet.
        let routes = json!([
            {"capability": "image.generate", "route": "img-a@acme", "extends": "img-b@acme"},
            {"capability": "image.generate", "route": "img-b@acme", "price": {"usd": 0.1}},
        ]);
        assert_eq!(
            table(routes).unwrap_err(),
            "routes.0.extends: no route img-b@acme serves image.generate"
        );
    }
}
