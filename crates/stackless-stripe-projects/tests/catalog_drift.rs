//! Strict drift detection: the committed fixture must contain nothing the typed
//! model does not cover. When this fails, the model (and fixture) need updating.

use stackless_stripe_projects::Catalog;

const FIXTURE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/catalog.json"
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
