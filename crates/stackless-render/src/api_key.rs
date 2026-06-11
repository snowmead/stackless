//! Render API key resolution (§4, render-api.ts's resolveRenderApiKey).
//!
//! Order: the `RENDER_API_KEY` env var, then a 0600 key file at
//! `<definition_dir>/.render-api-key` (the prior-art path). A missing
//! key is a clean fault naming both sources.

use std::path::Path;

use crate::error::RenderError;

pub const KEY_FILE: &str = ".render-api-key";

/// Resolve the key from the environment or the scoped key file next to
/// the definition. The env var wins so CI can inject without a file.
pub fn resolve(definition_dir: &Path) -> Result<String, RenderError> {
    if let Ok(key) = std::env::var("RENDER_API_KEY") {
        let key = key.trim().to_owned();
        if !key.is_empty() {
            return Ok(key);
        }
    }
    let key_file = definition_dir.join(KEY_FILE);
    if let Ok(contents) = std::fs::read_to_string(&key_file) {
        let key = contents.trim().to_owned();
        if !key.is_empty() {
            return Ok(key);
        }
    }
    Err(RenderError::ApiKeyMissing {
        key_file: key_file.display().to_string(),
    })
}
