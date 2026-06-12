//! Vercel API token resolution.
//!
//! Order: the `VERCEL_TOKEN` env var, then a 0600 key file at
//! `<definition_dir>/.vercel-token`. A missing token is a clean fault
//! naming both sources.

use std::path::Path;

use crate::error::VercelError;

pub const KEY_FILE: &str = ".vercel-token";

/// Resolve the token from the environment or the scoped key file next to
/// the definition. The env var wins so CI can inject without a file.
pub fn resolve(definition_dir: &Path) -> Result<String, VercelError> {
    if let Ok(token) = std::env::var("VERCEL_TOKEN") {
        let token = token.trim().to_owned();
        if !token.is_empty() {
            return Ok(token);
        }
    }
    let key_file = definition_dir.join(KEY_FILE);
    if let Ok(contents) = std::fs::read_to_string(&key_file) {
        let token = contents.trim().to_owned();
        if !token.is_empty() {
            return Ok(token);
        }
    }
    Err(VercelError::ApiKeyMissing {
        key_file: key_file.display().to_string(),
    })
}