//! Shared checkpoint kind policy for cloud substrates.
//!
//! Known ephemeral kinds (`action`, `source-ref` when it owns no checkout) are
//! Gone / Ok on observe/destroy. Unknown kinds fail closed so a typo or old
//! binary cannot drop a journal row while leaving a live resource.

use stackless_core::substrate::ACTION_RESOURCE_KIND;

/// Resource kinds that own nothing destructible on every cloud substrate.
pub fn is_ephemeral_resource_kind(kind: &str) -> bool {
    kind == ACTION_RESOURCE_KIND || kind == "source-ref"
}

/// Parse a checkpoint payload: empty → `Ok(None)` (legacy fallback);
/// non-empty malformed → `Err(detail)`; valid → `Ok(Some(T))`.
pub fn parse_payload<T: serde::de::DeserializeOwned>(payload: &str) -> Result<Option<T>, String> {
    let trimmed = payload.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    serde_json::from_str(trimmed)
        .map(Some)
        .map_err(|err| format!("malformed checkpoint payload: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Deserialize, PartialEq)]
    struct Sample {
        id: String,
    }

    #[test]
    fn empty_payload_is_none() {
        assert_eq!(parse_payload::<Sample>("").unwrap(), None);
        assert_eq!(parse_payload::<Sample>("  ").unwrap(), None);
    }

    #[test]
    fn valid_payload_parses() {
        let got = parse_payload::<Sample>(r#"{"id":"x"}"#).unwrap();
        assert_eq!(got, Some(Sample { id: "x".into() }));
    }

    #[test]
    fn malformed_nonempty_errors() {
        assert!(parse_payload::<Sample>("{").is_err());
    }

    #[test]
    fn ephemeral_kinds() {
        assert!(is_ephemeral_resource_kind("action"));
        assert!(is_ephemeral_resource_kind("source-ref"));
        assert!(!is_ephemeral_resource_kind("render-service"));
    }
}
