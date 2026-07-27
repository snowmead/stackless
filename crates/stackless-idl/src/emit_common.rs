//! Shared helpers for language emitters.

pub(crate) fn escape_str(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}
