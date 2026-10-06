//! The built-in default route table (spec/identity.md §7; spec/providers.md §10):
//! `routes/default.yaml`, embedded in the binary. The `grida-fx` command hands it to planning as
//! `PlanRequest::builtin_routes`; a `--routes` file leaves it out, and a project's `route_tables`
//! override its entries.
//!
//! What `tests/routes.rs` holds the table to:
//! - 16 routes over 10 capabilities, each fingerprint input (capability, model, provider,
//!   contract) as written there, so an accidental edit of an identity is caught;
//! - every capability is one of [`crate::capabilities::CAPABILITIES`], and every feature is in
//!   that capability's [`features`](crate::capabilities::Capability::features);
//! - every route is served by an adapter of this crate (`live::adapters`), whose kind matches the
//!   capability's shape, and every contract names the `adapter` (and `adapter_behavior`) that
//!   spec/providers.md §9 gives that adapter;
//! - prices parse, and the video route's tiers price as spec/identity.md §12 reads them.

use grida_fx_core::routes::RouteTable;

/// The table's text.
pub const DEFAULT_ROUTES: &str = include_str!("../routes/default.yaml");

/// How errors name the table.
pub const LABEL: &str = "the built-in route table";

/// The parsed table. It is checked by the crate's tests, so an error here is a build defect;
/// callers report it as an internal error.
pub fn default_table() -> Result<RouteTable, String> {
    let document = grida_fx_core::yaml::load(DEFAULT_ROUTES.as_bytes(), LABEL)
        .map_err(|error| format!("{LABEL}: {error}"))?;
    RouteTable::from_document(&document, LABEL).map_err(|error| error.message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_table_parses() {
        let table = default_table().unwrap();
        assert_eq!(table.entries.len(), 16);
        let capabilities: std::collections::BTreeSet<&str> =
            table.entries.keys().map(|(c, _)| c.as_str()).collect();
        assert_eq!(capabilities.len(), 10);
    }

    #[test]
    fn the_table_is_labelled_in_its_errors() {
        let document = grida_fx_core::yaml::load(b"fx: routes/v1\nroutes: 3\n", LABEL).unwrap();
        let error = RouteTable::from_document(&document, LABEL).unwrap_err();
        assert!(error.message.contains(LABEL), "{}", error.message);
    }
}
