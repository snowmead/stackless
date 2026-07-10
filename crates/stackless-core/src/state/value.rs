//! A driver-agnostic SQL value for the helper layer.

/// Bridges rusqlite and libsql params; only the variants the state store
/// actually binds (text, integers, null) — no blobs or reals are stored.
#[derive(Debug, Clone)]
pub(super) enum Value {
    Text(String),
    Int(i64),
    Null,
}

impl From<&str> for Value {
    fn from(v: &str) -> Self {
        Value::Text(v.to_owned())
    }
}
impl From<String> for Value {
    fn from(v: String) -> Self {
        Value::Text(v)
    }
}
impl From<i64> for Value {
    fn from(v: i64) -> Self {
        Value::Int(v)
    }
}
impl From<u32> for Value {
    fn from(v: u32) -> Self {
        Value::Int(v as i64)
    }
}

impl From<crate::types::Pid> for Value {
    fn from(v: crate::types::Pid) -> Self {
        Value::Int(v.get() as i64)
    }
}

impl From<crate::types::ProcessStartTime> for Value {
    fn from(v: crate::types::ProcessStartTime) -> Self {
        Value::Int(v.get() as i64)
    }
}

impl From<crate::types::DnsName> for Value {
    fn from(v: crate::types::DnsName) -> Self {
        Value::Text(v.into_inner())
    }
}
