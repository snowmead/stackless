//! Integration provider metadata consumed by the registry for validation,
//! output checking, and lifecycle dispatch. Each catalog adapter (Clerk, …)
//! implements [`Hostable`] once; the registry is built only from those impls.

use stackless_core::host::Host;

/// Whether an integration's config may vary per stack host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigScope {
    /// All settings live at `[integrations.<name>]`; host-key tables are rejected.
    GlobalOnly,
    /// `[integrations.<name>.<host>]` may override fields for hosts listed in
    /// [`IntegrationHosting::HostBound`].
    PerHost,
}

/// Where an integration's capability actually runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntegrationHosting {
    /// Provider-operated cloud (e.g. Clerk). Not tied to `--on`; always available
    /// when referenced. Must pair with [`ConfigScope::GlobalOnly`].
    Managed,
    /// Runs on or through specific stack hosts; `--on` must be in the host list.
    HostBound(&'static [Host]),
}

/// Metadata and compile-time constraints for one integration provider.
///
/// Implementors are registered in the integrations registry; validation and
/// dispatch never use free-form provider strings outside this table.
pub trait Hostable {
    /// Catalog name in `stackless.toml` (`provider = "clerk"`).
    const PROVIDER: &'static str;
    /// Runtime placement model — see [`IntegrationHosting`].
    const HOSTING: IntegrationHosting;
    /// TOML override policy — see [`ConfigScope`].
    const CONFIG_SCOPE: ConfigScope;
    /// Checkpoint `resource_kind` written by the provisioner.
    const RESOURCE_KIND: &'static str;
    /// Outputs available in `${integrations.<name>.<output>}` references.
    const OUTPUTS: &'static [&'static str];

    /// Compile-time guard: managed providers cannot accept per-host config.
    const VALIDATED: () = validate_hostable_pair(Self::HOSTING, Self::CONFIG_SCOPE);
}

/// Managed integrations must be globally configured.
const fn validate_hostable_pair(hosting: IntegrationHosting, scope: ConfigScope) {
    match (hosting, scope) {
        (IntegrationHosting::Managed, ConfigScope::GlobalOnly) => (),
        (IntegrationHosting::Managed, ConfigScope::PerHost) => {
            panic!("Managed integrations must use ConfigScope::GlobalOnly")
        }
        _ => (),
    }
}

/// Whether `host` is listed for a host-bound provider.
pub fn host_bound_supports(hosting: IntegrationHosting, host: Host) -> bool {
    match hosting {
        IntegrationHosting::Managed => true,
        IntegrationHosting::HostBound(hosts) => hosts.contains(&host),
    }
}

/// Hosts declared for a host-bound provider (empty for managed).
pub fn host_bound_hosts(hosting: IntegrationHosting) -> &'static [Host] {
    match hosting {
        IntegrationHosting::Managed => &[],
        IntegrationHosting::HostBound(hosts) => hosts,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_bound_supports_declared_hosts_only() {
        let hosting = IntegrationHosting::HostBound(&[Host::Local, Host::Render]);
        assert!(host_bound_supports(hosting, Host::Local));
        assert!(host_bound_supports(hosting, Host::Render));
        assert!(!host_bound_supports(hosting, Host::Vercel));
    }

    #[test]
    fn managed_supports_every_active_host_check() {
        let hosting = IntegrationHosting::Managed;
        for host in Host::ALL {
            assert!(host_bound_supports(hosting, *host));
        }
    }
}
