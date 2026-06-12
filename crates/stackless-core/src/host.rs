//! Stack **hosts** — where a disposable instance runs (`stackless up --on`).
//!
//! A host names a substrate implementation (`stackless-local`, `stackless-render`,
//! `stackless-vercel`). This is distinct from integration **hosting** (managed
//! provider cloud vs host-bound), which lives in `stackless-integrations`.

/// A stack hosting substrate selected at instance creation (`--on`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Host {
    /// Operator machine via `stackless-local`.
    Local,
    /// Render cloud via `stackless-render`.
    Render,
    /// Vercel cloud via `stackless-vercel`.
    Vercel,
}

impl Host {
    /// Every substrate the binary can dispatch to.
    pub const ALL: &'static [Host] = &[Host::Local, Host::Render, Host::Vercel];

    /// The CLI / TOML key for this host (`local`, `render`, `vercel`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Render => "render",
            Self::Vercel => "vercel",
        }
    }

    /// Parse a `--on` value or substrate block key.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "local" => Some(Self::Local),
            "render" => Some(Self::Render),
            "vercel" => Some(Self::Vercel),
            _ => None,
        }
    }

    /// Whether `key` names a registered host (for flatten-map field filtering).
    pub fn is_host_key(key: &str) -> bool {
        Self::parse(key).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::Host;

    #[test]
    fn all_hosts_round_trip() {
        for host in Host::ALL {
            assert_eq!(Host::parse(host.as_str()), Some(*host));
        }
    }
}