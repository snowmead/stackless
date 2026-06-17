//! The substrate registry — the one place the binary names hosting providers
//! (ground rule: core never names a substrate; the `Substrate` trait is the
//! only seam). Adding a hosting provider is one row here plus its own crate.

use stackless_core::substrate::Substrate;
use stackless_fly::{FlySubstrate, SUBSTRATE_NAME as FLY};
use stackless_local::{LocalSubstrate, SUBSTRATE_NAME as LOCAL};
use stackless_render::{RenderSubstrate, SUBSTRATE_NAME as RENDER};
use stackless_vercel::{SUBSTRATE_NAME as VERCEL, VercelSubstrate};

use crate::commands::SubstrateCtx;
use crate::error::CliError;

/// One registered substrate: its `--on` name and how to construct it from the
/// shared build context.
struct SubstrateInfo {
    name: &'static str,
    build: fn(SubstrateCtx) -> Result<Box<dyn Substrate>, CliError>,
}

static SUBSTRATES: &[SubstrateInfo] = &[
    SubstrateInfo {
        name: LOCAL,
        build: build_local,
    },
    SubstrateInfo {
        name: RENDER,
        build: build_render,
    },
    SubstrateInfo {
        name: VERCEL,
        build: build_vercel,
    },
    SubstrateInfo {
        name: FLY,
        build: build_fly,
    },
];

/// Every substrate name the binary can dispatch to.
pub(crate) fn known_names() -> Vec<&'static str> {
    SUBSTRATES.iter().map(|info| info.name).collect()
}

/// Error unless `name` is a registered substrate (used where no substrate is
/// constructed, e.g. `check`; `build` enforces the same for `up`/`down`).
pub(crate) fn ensure_known(name: &str) -> Result<(), CliError> {
    if SUBSTRATES.iter().any(|info| info.name == name) {
        Ok(())
    } else {
        Err(unknown(name))
    }
}

/// Construct a substrate by name, or report it unknown with the known set.
pub(crate) fn build(name: &str, ctx: SubstrateCtx) -> Result<Box<dyn Substrate>, CliError> {
    match SUBSTRATES.iter().find(|info| info.name == name) {
        Some(info) => (info.build)(ctx),
        None => Err(unknown(name)),
    }
}

fn unknown(name: &str) -> CliError {
    CliError::SubstrateUnknown {
        substrate: name.to_owned(),
        known: known_names().iter().map(|s| (*s).to_owned()).collect(),
    }
}

fn build_local(ctx: SubstrateCtx) -> Result<Box<dyn Substrate>, CliError> {
    Ok(Box::new(LocalSubstrate {
        proxy_port: stackless_daemon::proxy::proxy_port(),
        secrets: ctx.secrets,
        definition_dir: ctx.definition_dir,
    }))
}

fn build_render(ctx: SubstrateCtx) -> Result<Box<dyn Substrate>, CliError> {
    Ok(Box::new(RenderSubstrate::new(
        ctx.definition_dir,
        ctx.secrets,
        ctx.confirm_paid,
    )))
}

fn build_vercel(ctx: SubstrateCtx) -> Result<Box<dyn Substrate>, CliError> {
    Ok(Box::new(VercelSubstrate::new(
        ctx.definition_dir,
        ctx.secrets,
        ctx.confirm_paid,
    )))
}

fn build_fly(ctx: SubstrateCtx) -> Result<Box<dyn Substrate>, CliError> {
    Ok(Box::new(FlySubstrate::new(
        ctx.definition_dir,
        ctx.secrets,
        ctx.confirm_paid,
    )))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    /// Each crate owns its error codes; the binary is the one place that sees
    /// them all, so the workspace-wide no-collision check lives here. Adding a
    /// provider crate adds its `codes::ALL` to this list.
    #[test]
    fn error_codes_are_globally_unique() {
        let mut all: Vec<&str> = Vec::new();
        all.extend(stackless_core::fault::codes::ALL);
        all.extend(stackless_render::codes::ALL);
        all.extend(stackless_vercel::codes::ALL);
        all.extend(stackless_fly::codes::ALL);
        let unique: BTreeSet<&str> = all.iter().copied().collect();
        assert_eq!(
            unique.len(),
            all.len(),
            "duplicate error code across crates"
        );
    }
}
