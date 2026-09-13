#![allow(clippy::unwrap_used)]

use stackless_core::{def::StackDef, substrate::Substrate};
use stackless_local::LocalSubstrate;

#[test]
fn tcp_admission_rejects_unimplemented_container_and_cloud_routes() {
    let text =
        "[stack]\nname = 'tcp'\n[workloads.db]\nrun = 'server'\nhealth = { protocol = 'tcp' }\n";
    let def = StackDef::parse(text).unwrap();
    LocalSubstrate::default().validate(&def).unwrap();
    let container = StackDef::parse(&format!("{text}image = 'postgres:17'\n")).unwrap();
    let error = LocalSubstrate::default().validate(&container).unwrap_err();
    assert_eq!(&*error.code, "provider.unsupported");
    assert!(error.message.contains("TCP ingress"));
    let error = stackless_core::capabilities::Capabilities::cloud(true, true)
        .validate("fly", &def)
        .unwrap_err();
    assert_eq!(&*error.code, "provider.unsupported");
    assert!(error.message.contains("TCP services"));
}

#[test]
fn container_origin_references_require_a_reachable_peer() {
    let definition = r#"
[stack]
name = "test"
[workloads.client]
image = "alpine:3.21"
kind = "worker"
run = "sleep 300"
env = { API = "${services.api.origin}" }
[workloads.api]
health = { path = "/" }
run = "server"
"#;
    let def = StackDef::parse(definition).unwrap();
    let provider = LocalSubstrate::default();
    assert!(
        provider
            .validate_definition(&def)
            .unwrap_err()
            .message
            .contains("isolated container network")
    );
    let def = StackDef::parse(&format!("{definition}\nimage = 'alpine:3.21'\n")).unwrap();
    provider.validate_definition(&def).unwrap();
}

#[test]
fn image_references_reject_empty_or_whitespace_values() {
    for image in ["", " ", "alpine latest", "alpine\n"] {
        let definition = format!(
            "[stack]\nname = 'test'\n[jobs.check]\nimage = {}",
            serde_json::to_string(image).unwrap()
        );
        assert!(
            StackDef::parse(&definition)
                .unwrap()
                .validate_hosts(&["local"])
                .is_err()
        );
    }
}

#[test]
fn container_endpoint_aliases_cannot_escape_the_private_network() {
    let definition = r#"
[stack]
name = "test"
[workloads.client]
image = "alpine:3.21"
kind = "worker"
run = "sleep 300"
env = { API = "${endpoints.api.url}" }
[workloads.api]
image = "alpine:3.21"
health = { path = "/" }
[endpoints.api]
workload = "api"
"#;
    let provider = LocalSubstrate::default();
    let def = StackDef::parse(definition).unwrap();
    provider.validate_definition(&def).unwrap();
    for invalid in [
        format!("{definition}\nurl = 'https://external.example.test'"),
        definition.replace(
            "[workloads.api]\nimage = \"alpine:3.21\"",
            "[workloads.api]\nrun = 'server'",
        ),
    ] {
        let def = StackDef::parse(&invalid).unwrap();
        assert!(
            provider
                .validate_definition(&def)
                .unwrap_err()
                .message
                .contains("isolated container network")
        );
    }
}
