//! The family's bounded, opt-in billing policy (src/openai_compatible/README.md, "Cost").
//! The policy belongs to the route contract, so its meaning is part of the route fingerprint.

use crate::transport::HttpResponse;
use crate::wire;
use grida_fx_core::money::Usd;
use serde_json::{Map, Value};

/// A missing report permits a documented unbilled-status rule; an unreadable report does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Cost {
    Missing,
    Known(Usd),
    Unknown,
}

impl Cost {
    /// The charge the engine can settle; either kind of uncertainty charges the whole hold.
    pub(crate) fn reported(self) -> Option<Usd> {
        match self {
            Cost::Known(cost) => Some(cost),
            Cost::Missing | Cost::Unknown => None,
        }
    }
}

/// Only a full USD `usage.cost` report and explicitly unbilled refusal statuses are supported.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct BillingPolicy {
    pub(crate) reads_cost: bool,
    unbilled_statuses: Vec<u16>,
}

impl BillingPolicy {
    /// Reads `contract.billing`; absent or null settings keep the conservative default.
    /// The caller adds the adapter's ordinary contract-refusal prefix to an error.
    pub(crate) fn from_contract(contract: &Value) -> Result<Self, String> {
        let Some(billing) = contract.get("billing").filter(|value| !value.is_null()) else {
            return Ok(Self::default());
        };
        let billing = billing
            .as_object()
            .ok_or_else(|| "billing is an object".to_string())?;
        for member in billing.keys() {
            if !["cost", "unbilled_statuses"].contains(&member.as_str()) {
                return Err(format!("billing takes no member {member}"));
            }
        }
        let reads_cost = match billing.get("cost").filter(|value| !value.is_null()) {
            None => false,
            Some(Value::String(path)) if path == "usage.cost" => true,
            Some(_) => return Err("billing.cost is usage.cost".into()),
        };
        let mut unbilled_statuses = Vec::new();
        if let Some(statuses) = billing
            .get("unbilled_statuses")
            .filter(|value| !value.is_null())
        {
            let statuses = statuses
                .as_array()
                .ok_or_else(|| "billing.unbilled_statuses is an array".to_string())?;
            for status in statuses {
                let Some(status) = status
                    .as_u64()
                    .filter(|status| (400..500).contains(status) && ![408, 429].contains(status))
                else {
                    return Err(
                        "billing.unbilled_statuses contains only integers from 400 through 499, excluding 408 and 429"
                            .into(),
                    );
                };
                let status = status as u16;
                if unbilled_statuses.contains(&status) {
                    return Err(format!("billing.unbilled_statuses repeats HTTP {status}"));
                }
                unbilled_statuses.push(status);
            }
        }
        Ok(Self {
            reads_cost,
            unbilled_statuses,
        })
    }

    /// The cost of a parsed response body, before any HTTP-status inference.
    pub(crate) fn body_cost(&self, body: &Map<String, Value>) -> Cost {
        if !self.reads_cost {
            return Cost::Missing;
        }
        let Some(usage) = body.get("usage").filter(|value| !value.is_null()) else {
            return Cost::Missing;
        };
        let Some(usage) = usage.as_object() else {
            return Cost::Unknown;
        };
        let Some(cost) = usage.get("cost").filter(|value| !value.is_null()) else {
            return Cost::Missing;
        };
        match wire::usd_ceil(cost) {
            Some(cost) => Cost::Known(cost),
            None => Cost::Unknown,
        }
    }

    /// A status response's charge: reported cost wins, then an explicitly unbilled status.
    /// An invalid nonempty body cannot establish that the provider reported no cost. Read it
    /// strictly: a duplicate key must not replace a possible charge with a missing value.
    pub(crate) fn response_cost(&self, response: &HttpResponse) -> Cost {
        let unbilled = self.unbilled_statuses.contains(&response.status);
        if !self.reads_cost && !unbilled {
            return Cost::Missing;
        }
        let cost = if response.body.is_empty() {
            Cost::Missing
        } else {
            let body = std::str::from_utf8(&response.body)
                .ok()
                .and_then(|text| grida_fx_core::value::parse_json(text).ok());
            match body {
                Some(Value::Object(body)) => self.body_cost(&body),
                _ => Cost::Unknown,
            }
        };
        match cost {
            Cost::Missing if unbilled => Cost::Known(Usd::ZERO),
            _ => cost,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn policy(billing: Value) -> BillingPolicy {
        BillingPolicy::from_contract(&json!({"billing": billing})).unwrap()
    }

    #[test]
    fn absent_and_null_settings_keep_the_default() {
        for contract in [
            json!({}),
            json!({"billing": null}),
            json!({"billing": {}}),
            json!({"billing": {"cost": null, "unbilled_statuses": null}}),
        ] {
            assert_eq!(
                BillingPolicy::from_contract(&contract).unwrap(),
                BillingPolicy::default()
            );
        }
    }

    #[test]
    fn reported_money_uses_the_existing_exact_rounding() {
        let policy = policy(json!({"cost": "usage.cost"}));
        for (value, expected) in [
            (json!(0), Cost::Known(Usd::ZERO)),
            (json!(0.210835), Cost::Known(Usd(210_835))),
            (json!(0.0000001), Cost::Known(Usd(1))),
            (json!(0.00012345), Cost::Known(Usd(124))),
            (json!(null), Cost::Missing),
            (json!("0"), Cost::Unknown),
            (json!(false), Cost::Unknown),
            (json!(-1), Cost::Unknown),
            (json!(1e30), Cost::Unknown),
            (json!({}), Cost::Unknown),
        ] {
            let body = json!({"usage": {"cost": value}});
            assert_eq!(policy.body_cost(body.as_object().unwrap()), expected);
        }
        for body in [json!({}), json!({"usage": null}), json!({"usage": {}})] {
            assert_eq!(policy.body_cost(body.as_object().unwrap()), Cost::Missing);
        }
        for usage in [json!(1), json!([]), json!("not usage")] {
            let body = json!({"usage": usage});
            assert_eq!(policy.body_cost(body.as_object().unwrap()), Cost::Unknown);
        }
    }

    #[test]
    fn unbilled_rules_cannot_hide_invalid_reports_or_unreadable_bodies() {
        let policy = policy(json!({"cost": "usage.cost", "unbilled_statuses": [400]}));
        for (body, expected) in [
            (json!({}), Cost::Known(Usd::ZERO)),
            (json!({"usage": {"cost": null}}), Cost::Known(Usd::ZERO)),
            (json!({"usage": {"cost": 0.2}}), Cost::Known(Usd(200_000))),
            (json!({"usage": {"cost": "0"}}), Cost::Unknown),
            (json!({"usage": {"cost": -1}}), Cost::Unknown),
            (json!([]), Cost::Unknown),
            (json!(null), Cost::Unknown),
        ] {
            assert_eq!(
                policy.response_cost(&HttpResponse::json(400, &body)),
                expected
            );
        }
        assert_eq!(
            policy.response_cost(&HttpResponse::new(400, Vec::new())),
            Cost::Known(Usd::ZERO)
        );
        assert_eq!(
            policy.response_cost(&HttpResponse::new(400, b"not JSON".to_vec())),
            Cost::Unknown
        );
        assert_eq!(
            policy.response_cost(&HttpResponse::new(401, Vec::new())),
            Cost::Missing
        );
    }

    #[test]
    fn status_rules_work_without_enabling_a_cost_reader() {
        let policy = policy(json!({"unbilled_statuses": [422]}));
        assert!(!policy.reads_cost);
        let body = json!({"usage": {"cost": 0.5}});
        assert_eq!(policy.body_cost(body.as_object().unwrap()), Cost::Missing);
        assert_eq!(
            policy.response_cost(&HttpResponse::json(422, &body)),
            Cost::Known(Usd::ZERO)
        );
        assert_eq!(
            policy.response_cost(&HttpResponse::new(422, b"bad JSON".to_vec())),
            Cost::Unknown
        );
    }

    #[test]
    fn duplicate_fields_and_excessive_nesting_cannot_hide_a_charge() {
        let policy = policy(json!({"cost": "usage.cost", "unbilled_statuses": [400]}));
        for body in [
            br#"{"usage":{"cost":0.2,"cost":null}}"#.to_vec(),
            br#"{"usage":{"cost":0.2},"usage":{"cost":null}}"#.to_vec(),
            br#"{"usage":{"cost":0.2,"\u0063ost":null}}"#.to_vec(),
            vec![0xff],
        ] {
            assert_eq!(
                policy.response_cost(&HttpResponse::new(400, body)),
                Cost::Unknown
            );
        }
        // The strict reader bounds nesting before it visits untrusted values recursively.
        let levels = grida_fx_core::value::MAX_DEPTH + 1;
        let body = format!(
            "{{\"usage\":{{\"cost\":null}},\"meta\":{}0{}}}",
            "[".repeat(levels),
            "]".repeat(levels)
        );
        assert_eq!(
            policy.response_cost(&HttpResponse::new(400, body.into_bytes())),
            Cost::Unknown
        );
    }
}
