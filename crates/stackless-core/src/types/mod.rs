//! Semantic newtypes: validate once at construction, trust downstream.

mod error;

pub use error::TypeError;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub fn dns_safe(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 63
        && name.starts_with(|c: char| c.is_ascii_lowercase())
        && !name.ends_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn proxy_host_valid(host: &str) -> Result<(), &'static str> {
    if host.is_empty() {
        return Err("empty");
    }
    if !host.is_ascii() {
        return Err("non-ASCII");
    }
    if host.contains('\0') {
        return Err("NUL byte");
    }
    if !host.ends_with(".localhost") {
        return Err("must end with .localhost");
    }
    Ok(())
}

macro_rules! numeric_newtype {
    ($name:ident, $inner:ty, $error:ident, $validate:expr) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub struct $name($inner);

        impl $name {
            pub fn try_new(raw: $inner) -> Result<Self, TypeError> {
                if $validate(raw) {
                    Ok(Self(raw))
                } else {
                    Err(TypeError::$error { value: raw })
                }
            }

            pub fn from_os(raw: $inner) -> Self {
                Self(raw)
            }

            pub fn get(self) -> $inner {
                self.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                self.0.serialize(serializer)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let raw = <$inner>::deserialize(deserializer)?;
                Self::try_new(raw).map_err(serde::de::Error::custom)
            }
        }
    };
}

numeric_newtype!(Pid, u32, InvalidPid, |v| v != 0);
numeric_newtype!(ProcessStartTime, u64, InvalidProcessStartTime, |v| v != 0);
numeric_newtype!(TcpPort, u16, InvalidTcpPort, |v| (1..=65535).contains(&v));
numeric_newtype!(HttpStatus, u16, InvalidHttpStatus, |v| (100..=599)
    .contains(&v));

impl HttpStatus {
    pub const OK: Self = Self(200);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProtocolVersion(u32);

impl ProtocolVersion {
    pub const V1: Self = Self(1);

    pub fn try_new(raw: u32) -> Result<Self, TypeError> {
        if raw == Self::V1.0 {
            Ok(Self(raw))
        } else {
            Err(TypeError::InvalidProtocolVersion { value: raw })
        }
    }

    pub fn get(self) -> u32 {
        self.0
    }
}

impl Serialize for ProtocolVersion {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ProtocolVersion {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = u32::deserialize(deserializer)?;
        Self::try_new(raw).map_err(serde::de::Error::custom)
    }
}

macro_rules! string_newtype {
    ($name:ident, $error:ident, $validate:expr, $deserialize:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(String);

        impl $name {
            pub fn try_new(value: impl Into<String>) -> Result<Self, TypeError> {
                let value = value.into();
                match $validate(&value) {
                    Ok(()) => Ok(Self(value)),
                    Err(detail) => Err(TypeError::$error {
                        value,
                        detail: detail.to_owned(),
                    }),
                }
            }

            /// Wrap a value already validated elsewhere (e.g. TOML → validate()).
            pub fn from_stored(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }

            pub fn into_inner(self) -> String {
                self.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                self.0.serialize(serializer)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                $deserialize(deserializer)
            }
        }
    };
}

string_newtype!(
    DnsName,
    InvalidDnsName,
    |v: &str| {
        if dns_safe(v) {
            Ok(())
        } else {
            Err("must be DNS-safe (lowercase alphanumeric and hyphens, 1-63 chars)")
        }
    },
    deserialize_dns_name
);

string_newtype!(
    ProxyHost,
    InvalidProxyHost,
    proxy_host_valid,
    deserialize_validated_proxy_host
);

fn deserialize_dns_name<'de, D: Deserializer<'de>>(deserializer: D) -> Result<DnsName, D::Error> {
    let value = String::deserialize(deserializer)?;
    Ok(DnsName::from_stored(value))
}

fn deserialize_validated_proxy_host<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<ProxyHost, D::Error> {
    let value = String::deserialize(deserializer)?;
    ProxyHost::try_new(value).map_err(serde::de::Error::custom)
}

impl LogPath {
    pub fn try_new(value: impl Into<String>) -> Result<Self, TypeError> {
        let value = value.into();
        if value.is_empty() {
            return Err(TypeError::InvalidLogPath {
                value,
                detail: "empty".into(),
            });
        }
        if value.contains('\0') {
            return Err(TypeError::InvalidLogPath {
                value,
                detail: "NUL byte".into(),
            });
        }
        Ok(Self(value))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LogPath(String);

impl LogPath {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

impl std::fmt::Display for LogPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for LogPath {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for LogPath {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Self::try_new(value).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ContainerId(String);

impl ContainerId {
    pub fn try_new(value: impl Into<String>) -> Result<Self, TypeError> {
        let value = value.into();
        if value.is_empty() {
            return Err(TypeError::InvalidContainerId { value });
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

impl std::fmt::Display for ContainerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for ContainerId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ContainerId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Self::try_new(value).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pid_rejects_zero() {
        assert!(Pid::try_new(0).is_err());
        assert_eq!(Pid::from_os(42).get(), 42);
    }

    #[test]
    fn tcp_port_range() {
        assert!(TcpPort::try_new(0).is_err());
        assert_eq!(TcpPort::try_new(4444).unwrap().get(), 4444);
        assert_eq!(TcpPort::try_new(65535).unwrap().get(), 65535);
    }

    #[test]
    fn proxy_host_requires_localhost_suffix() {
        assert!(ProxyHost::try_new("api.dev.localhost").is_ok());
        assert!(ProxyHost::try_new("api.example.com").is_err());
    }

    #[test]
    fn dns_name_matches_dns_safe() {
        assert!(DnsName::try_new("atto").is_ok());
        assert!(DnsName::try_new("API").is_err());
    }
}
