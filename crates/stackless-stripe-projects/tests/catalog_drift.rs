//! Strict drift detection: the committed fixture must contain nothing the typed
//! model does not cover. When this fails, the model (and fixture) need updating.

use stackless_stripe_projects::Catalog;

const FIXTURE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/catalog.json"
));

const SURFACE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/command-surface.txt"
));

const PINNED_VERSION: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/plugin-version.txt"
));

#[test]
fn fixture_parses_into_typed_catalog() {
    let catalog =
        Catalog::from_json_envelope(FIXTURE).expect("fixture is a valid catalog envelope");
    assert!(
        catalog.services.len() > 100,
        "expected the full catalog, got {} services",
        catalog.services.len()
    );
    assert!(
        catalog.lookup("clerk/auth").is_some(),
        "clerk/auth should be in the catalog"
    );
    assert!(catalog.lookup("render/postgres").is_some());
}

#[test]
fn fixture_has_no_unmodeled_drift() {
    let catalog =
        Catalog::from_json_envelope(FIXTURE).expect("fixture is a valid catalog envelope");
    let report = catalog.drift_report();
    assert!(
        report.is_empty(),
        "catalog drift — model does not fully cover the wire format:\n{}",
        report.join("\n")
    );
}

/// Offline coherence of the plugin-pinned snapshots — runs in the every-PR
/// hermetic gate (no `stripe` needed). The live bless (`STRIPE_PROJECTS_REFRESH=1`)
/// produces these; this guards against hand-edits, a stale surface after a
/// `TRAVERSAL` change, a version-header mismatch, or a command added upstream
/// but not tracked.
#[test]
fn fixtures_are_coherent() {
    use stackless_stripe_projects::{command_separators, parse_header_version};

    let pinned = PINNED_VERSION.trim();
    assert!(!pinned.is_empty(), "plugin-version.txt is empty");

    // (a) The surface header pins the same version as plugin-version.txt.
    assert_eq!(
        parse_header_version(SURFACE).as_deref(),
        Some(pinned),
        "command-surface.txt header version disagrees with plugin-version.txt — re-run `mise run stripe-refresh`"
    );

    // (b) The committed block separators are exactly what the code's TRAVERSAL
    //     produces, in order — catches hand-edits or a TRAVERSAL change with no
    //     re-bless.
    let committed: Vec<&str> = SURFACE
        .lines()
        .filter(|l| l.starts_with("===== "))
        .collect();
    let expected = command_separators();
    assert_eq!(
        committed,
        expected.iter().map(String::as_str).collect::<Vec<_>>(),
        "command-surface.txt blocks differ from the code's TRAVERSAL — re-run `mise run stripe-refresh`"
    );

    // (c) Every command the top-level banner advertises has a captured block, so
    //     a command shipped upstream but not added to TRAVERSAL fails CI.
    for cmd in banner_commands(SURFACE) {
        let sep = format!("===== stripe projects {cmd} --help =====");
        assert!(
            committed.contains(&sep.as_str()),
            "banner advertises `{cmd}` but it has no captured block — add it to TRAVERSAL in src/surface.rs and re-bless"
        );
    }
}

/// First tokens of the commands listed in the top-level `--help` banner block.
/// A command line is indented exactly two spaces; wrapped description lines are
/// indented further, and flag/example lines start with `-`/`$`, so both are
/// excluded.
fn banner_commands(surface: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_banner = false;
    for line in surface.lines() {
        if line.starts_with("===== ") {
            if in_banner {
                break; // banner is the first block only
            }
            in_banner = line == "===== stripe projects --help =====";
            continue;
        }
        if !in_banner {
            continue;
        }
        let Some(rest) = line.strip_prefix("  ") else {
            continue;
        };
        if rest.starts_with(' ') {
            continue; // wrapped continuation line
        }
        let token = rest.split_whitespace().next().unwrap_or_default();
        if !token.is_empty()
            && !token.starts_with('-')
            && token.chars().all(|c| c.is_ascii_lowercase() || c == '-')
        {
            out.push(token.to_owned());
        }
    }
    out
}
