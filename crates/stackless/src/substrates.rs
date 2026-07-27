//! The substrate registry — the one place the binary names hosting providers
//! (ground rule: core never names a substrate; the `Substrate` trait is the
//! only seam). Adding a hosting provider is one row here plus its own crate.

use stackless_core::substrate::Substrate;

use crate::client::SubstrateCtx;
use crate::error::Error;

/// One registered substrate: its `--on` name and how to construct it from the
/// shared build context.
struct SubstrateInfo {
    name: &'static str,
    build: fn(SubstrateCtx) -> Result<Box<dyn Substrate>, Error>,
}

static SUBSTRATES: &[SubstrateInfo] = &[
    SubstrateInfo {
        name: stackless_local::SUBSTRATE_NAME,
        build: build_local,
    },
    SubstrateInfo {
        name: stackless_render::SUBSTRATE_NAME,
        build: build_render,
    },
    SubstrateInfo {
        name: stackless_vercel::SUBSTRATE_NAME,
        build: build_vercel,
    },
    SubstrateInfo {
        name: stackless_fly::SUBSTRATE_NAME,
        build: build_fly,
    },
    SubstrateInfo {
        name: stackless_netlify::SUBSTRATE_NAME,
        build: build_netlify,
    },
    SubstrateInfo {
        name: stackless_railway::SUBSTRATE_NAME,
        build: build_railway,
    },
    SubstrateInfo {
        name: stackless_cloudflare::SUBSTRATE_NAME,
        build: build_cloudflare,
    },
    SubstrateInfo {
        name: stackless_wordpress::SUBSTRATE_NAME,
        build: build_wordpress,
    },
    SubstrateInfo {
        name: stackless_laravel_cloud::SUBSTRATE_NAME,
        build: build_laravel_cloud,
    },
    SubstrateInfo {
        name: stackless_gitlab::SUBSTRATE_NAME,
        build: build_gitlab,
    },
];

/// Every substrate name the binary can dispatch to.
pub(crate) fn known_names() -> Vec<&'static str> {
    SUBSTRATES.iter().map(|info| info.name).collect()
}

/// Error unless `name` is a registered substrate (used where no substrate is
/// constructed, e.g. `check`; `build` enforces the same for `up`/`down`).
pub(crate) fn ensure_known(name: &str) -> Result<(), Error> {
    if SUBSTRATES.iter().any(|info| info.name == name) {
        Ok(())
    } else {
        Err(unknown(name))
    }
}

/// Construct a substrate by name, or report it unknown with the known set.
pub(crate) fn build(name: &str, ctx: SubstrateCtx) -> Result<Box<dyn Substrate>, Error> {
    match SUBSTRATES.iter().find(|info| info.name == name) {
        Some(info) => (info.build)(ctx),
        None => Err(unknown(name)),
    }
}

fn unknown(name: &str) -> Error {
    Error::SubstrateUnknown {
        substrate: name.to_owned(),
        known: known_names().iter().map(|s| (*s).to_owned()).collect(),
    }
}

fn build_local(ctx: SubstrateCtx) -> Result<Box<dyn Substrate>, Error> {
    Ok(Box::new(stackless_local::LocalSubstrate {
        proxy_port: ctx.proxy_port,
        state_root: ctx.state_root,
        secrets: ctx.secrets,
        definition_dir: ctx.definition_dir,
    }))
}

fn build_render(ctx: SubstrateCtx) -> Result<Box<dyn Substrate>, Error> {
    Ok(Box::new(stackless_render::RenderSubstrate::new(
        ctx.definition_dir,
        ctx.secrets,
        ctx.confirm_paid,
    )))
}

fn build_vercel(ctx: SubstrateCtx) -> Result<Box<dyn Substrate>, Error> {
    Ok(Box::new(stackless_vercel::VercelSubstrate::new(
        ctx.definition_dir,
        ctx.secrets,
        ctx.confirm_paid,
    )))
}

fn build_fly(ctx: SubstrateCtx) -> Result<Box<dyn Substrate>, Error> {
    Ok(Box::new(stackless_fly::FlySubstrate::new(
        ctx.definition_dir,
        ctx.secrets,
        ctx.confirm_paid,
    )))
}

fn build_netlify(ctx: SubstrateCtx) -> Result<Box<dyn Substrate>, Error> {
    Ok(Box::new(stackless_netlify::NetlifySubstrate::new(
        ctx.definition_dir,
        ctx.secrets,
        ctx.confirm_paid,
    )))
}

fn build_laravel_cloud(ctx: SubstrateCtx) -> Result<Box<dyn Substrate>, Error> {
    Ok(Box::new(
        stackless_laravel_cloud::LaravelCloudSubstrate::new(
            ctx.definition_dir,
            ctx.secrets,
            ctx.confirm_paid,
        ),
    ))
}

fn build_railway(ctx: SubstrateCtx) -> Result<Box<dyn Substrate>, Error> {
    Ok(Box::new(stackless_railway::RailwaySubstrate::new(
        ctx.definition_dir,
        ctx.secrets,
        ctx.confirm_paid,
    )))
}

fn build_cloudflare(ctx: SubstrateCtx) -> Result<Box<dyn Substrate>, Error> {
    Ok(Box::new(stackless_cloudflare::CloudflareSubstrate::new(
        ctx.definition_dir,
        ctx.secrets,
        ctx.confirm_paid,
    )))
}

fn build_wordpress(ctx: SubstrateCtx) -> Result<Box<dyn Substrate>, Error> {
    Ok(Box::new(stackless_wordpress::WordPressSubstrate::new(
        ctx.definition_dir,
        ctx.secrets,
        ctx.confirm_paid,
    )))
}

fn build_gitlab(ctx: SubstrateCtx) -> Result<Box<dyn Substrate>, Error> {
    Ok(Box::new(stackless_gitlab::GitLabSubstrate::new(
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
        all.extend(stackless_netlify::codes::ALL);
        all.extend(stackless_railway::codes::ALL);
        all.extend(stackless_cloudflare::codes::ALL);
        all.extend(stackless_wordpress::codes::ALL);
        all.extend(stackless_laravel_cloud::codes::ALL);
        all.extend(stackless_gitlab::codes::ALL);
        let unique: BTreeSet<&str> = all.iter().copied().collect();
        assert_eq!(
            unique.len(),
            all.len(),
            "duplicate error code across crates"
        );
    }
}
