//! The OpenAI-compatible family's explicit wire choices. Defaults keep the original request;
//! a route's contract, and therefore its fingerprint, holds every opt-in.

use crate::openrouter::structured::IMAGE_DETAILS;
use serde_json::Value;

/// Wire settings shared by the family's structured output and agent turns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WirePolicy {
    pub(crate) image_detail: Option<String>,
    pub(crate) pictures_unchanged: bool,
    pub(crate) token_limit_field: &'static str,
}

impl WirePolicy {
    /// Reads the values after the caller checks the adapter identity and allowed members.
    /// Errors are details for the caller's `not_served` refusal.
    pub(crate) fn from_contract(contract: &Value, capability: &str) -> Result<Self, String> {
        let image_detail = match contract.get("image_detail") {
            None | Some(Value::Null) => None,
            Some(Value::String(detail)) if IMAGE_DETAILS.contains(&detail.as_str()) => {
                Some(detail.clone())
            }
            Some(_) => {
                return Err(format!(
                    "image_detail is one of {}",
                    IMAGE_DETAILS.join(", ")
                ));
            }
        };
        let pictures_unchanged = match contract.get("pictures") {
            None => false,
            Some(_) if capability != super::structured::CAPABILITY => {
                return Err("the contract takes no member pictures".into());
            }
            Some(Value::Null) => false,
            Some(Value::String(pictures)) if pictures == "unchanged" => true,
            Some(_) => return Err("pictures is \"unchanged\" or absent".into()),
        };
        let token_limit_field = match contract.get("token_limit_field") {
            None | Some(Value::Null) => "max_tokens",
            Some(Value::String(field)) if field == "max_tokens" => "max_tokens",
            Some(Value::String(field)) if field == "max_completion_tokens" => {
                "max_completion_tokens"
            }
            Some(_) => {
                return Err("token_limit_field is one of max_tokens, max_completion_tokens".into());
            }
        };
        Ok(Self {
            image_detail,
            pictures_unchanged,
            token_limit_field,
        })
    }
}
