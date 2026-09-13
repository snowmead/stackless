//! Endpoint aliases bind URLs without changing the workload's native origin.
#![allow(clippy::unwrap_used)]

use stackless_core::def::{DefError, EndpointSource, Namespace, StackDef, interp};
use stackless_core::fault::Fault;

const DEFINITION: &str = r#"
[stack]
name = "endpoints"
[workloads.api]
run = "server"
health = { path = "/" }
[workloads.web]
run = "server"
health = { path = "/" }
env = { API = "${endpoints.public-api.url}" }
[endpoints.public-api]
workload = "api"
url = "https://api.example.test/v1"
[endpoints.internal-api]
workload = "api"
"#;

#[test]
fn tcp_endpoints_require_matching_urls_and_late_output_dependencies() {
    let text = r#"
[stack]
name = "tcp"
[workloads.db]
run = "server"
health = { protocol = "tcp" }
[jobs.client]
run = "client"
env = { DB = "${endpoints.database.url}" }
depends_on = { db = "ready" }
[endpoints.database]
workload = "db"
"#;
    let def = StackDef::parse(text).unwrap();
    def.validate_hosts(&["local"]).unwrap();
    let plan = def.execution_plan("local", true).unwrap();
    assert!(plan.dependencies["job:client"].contains("start:db"));
    assert!(plan.dependencies["job:client"].contains("health:db"));
    let http = StackDef::parse(&text.replace("protocol = \"tcp\"", "path = '/'")).unwrap();
    assert!(
        !http.execution_plan("local", true).unwrap().dependencies["job:client"]
            .contains("start:db")
    );
    let explicit = StackDef::parse(&format!("{text}\nurl = 'tcp://[::1]:5432'")).unwrap();
    explicit.validate_hosts(&["local"]).unwrap();
    assert!(
        !explicit.execution_plan("local", true).unwrap().dependencies["job:client"]
            .contains("start:db")
    );
    for url in [
        "http://127.0.0.1:5432",
        "tcp://127.0.0.1",
        "tcp://127.0.0.1:0",
        "tcp://u:p@host:5432",
        "tcp://host:5432/path",
        "tcp://host:5432?query",
        "tcp://host:5432#fragment",
    ] {
        let invalid = StackDef::parse(&format!("{text}\nurl = '{url}'")).unwrap();
        assert!(invalid.validate_hosts(&["local"]).is_err(), "{url}");
    }
    for health in [
        "protocol = 'tcp', path = '/'",
        "protocol = 'tcp', status = 503",
        "protocol = 'tcp', contains = 'ok'",
        "protocol = 'udp'",
        "",
        "path = 'relative'",
    ] {
        assert!(
            StackDef::parse(&text.replace("protocol = \"tcp\"", health)).is_err(),
            "{health}"
        );
    }
    let root =
        StackDef::parse(&text.replace("run = \"server\"", "run = 'server'\nroot_origin = true"))
            .unwrap();
    assert!(root.validate_hosts(&["local"]).is_err());
    let self_url = StackDef::parse(&text.replace(
        "run = \"server\"",
        "run = 'server'\nenv = { SELF = '${services.db.origin}' }",
    ))
    .unwrap();
    assert!(self_url.execution_plan("local", true).is_err());
    let no_listener = StackDef::parse(
        "[stack]\nname = 'worker'\n[workloads.worker]\nkind = 'worker'\nrun = 'consume'\nenv = { SELF = '${services.worker.origin}' }",
    ).unwrap();
    assert!(no_listener.validate_hosts(&["local"]).is_err());
}

#[test]
fn named_urls_preserve_aliases_and_report_unavailable_provider_outputs() {
    let def = StackDef::parse(DEFINITION).unwrap();
    def.validate_hosts(&["local"]).unwrap();
    let mut namespace = Namespace::default();
    namespace.bind_endpoints(&def);
    assert_eq!(
        interp::resolve("${endpoints.public-api.url}", &namespace, "env").unwrap(),
        "https://api.example.test/v1"
    );
    assert_eq!(
        interp::resolve("${endpoints.internal-api.url}", &namespace, "env")
            .unwrap_err()
            .code(),
        "def.resolve.endpoint_unavailable"
    );
    namespace
        .service_origins
        .insert("api".into(), "http://api.demo.localhost:4444".into());
    namespace.bind_endpoints(&def);
    assert_eq!(
        interp::resolve(
            "${services.api.origin}|${endpoints.internal-api.url}|${endpoints.public-api.url}",
            &namespace,
            "env"
        )
        .unwrap(),
        "http://api.demo.localhost:4444|http://api.demo.localhost:4444|https://api.example.test/v1"
    );
    let bindings = def.resolve_endpoints(&namespace.service_origins);
    assert_eq!(bindings["public-api"].source, EndpointSource::Declared);
    assert_eq!(bindings["internal-api"].source, EndpointSource::Provider);
    namespace.service_origins.clear();
    namespace.bind_endpoints(&def);
    assert!(!namespace.endpoint_urls.contains_key("internal-api"));
}

#[test]
fn endpoint_output_dependencies_do_not_erase_readiness_or_native_origin_dependencies() {
    let def = StackDef::parse(DEFINITION).unwrap();
    let plan = def.execution_plan("cloud", false).unwrap();
    assert!(!plan.dependencies["start:web"].contains("start:api"));
    let native = StackDef::parse(
        &DEFINITION.replace("${endpoints.public-api.url}", "${services.api.origin}"),
    )
    .unwrap();
    assert!(
        native.execution_plan("cloud", false).unwrap().dependencies["start:web"]
            .contains("start:api")
    );
    let dynamic = StackDef::parse(&DEFINITION.replace(
        "${endpoints.public-api.url}",
        "${endpoints.internal-api.url}",
    ))
    .unwrap();
    assert!(
        dynamic.execution_plan("cloud", false).unwrap().dependencies["start:web"]
            .contains("start:api")
    );
    assert!(
        !dynamic.execution_plan("local", true).unwrap().dependencies["start:web"]
            .contains("start:api")
    );
    let ready = StackDef::parse(&DEFINITION.replace(
        "[workloads.web]",
        "[workloads.web]\ndepends_on = { api = 'ready' }",
    ))
    .unwrap();
    assert!(
        ready.execution_plan("cloud", false).unwrap().dependencies["start:web"]
            .contains("health:api")
    );
    let self_dynamic = StackDef::parse(
        &DEFINITION
            .replace(
                "${endpoints.public-api.url}",
                "${endpoints.internal-api.url}",
            )
            .replace("workload = \"api\"", "workload = \"web\""),
    )
    .unwrap();
    assert!(matches!(
        self_dynamic.execution_plan("cloud", false),
        Err(DefError::WiringCycle { .. })
    ));
    self_dynamic.execution_plan("local", true).unwrap();
    let self_origin = StackDef::parse(
        &DEFINITION.replace("${endpoints.public-api.url}", "${services.web.origin}"),
    )
    .unwrap();
    assert!(matches!(
        self_origin.execution_plan("cloud", false),
        Err(DefError::WiringCycle { .. })
    ));
    self_origin.execution_plan("local", true).unwrap();
}

#[test]
fn endpoint_validation_rejects_unknown_targets_non_http_workloads_and_invalid_urls() {
    for invalid in [
        "https://",
        "https:///",
        "https://user:password@example.test",
        "file:///tmp/socket",
        "https://host.test/a b",
        "https://host.test:bad",
    ] {
        let def =
            StackDef::parse(&DEFINITION.replace("https://api.example.test/v1", invalid)).unwrap();
        assert!(def.validate_hosts(&["local"]).is_err(), "{invalid}");
    }
    for invalid in [
        format!(
            "{DEFINITION}\n[integrations.auth]\nprovider = 'clerk'\napp_name = '${{endpoints.public-api.url}}'"
        ),
        DEFINITION.replace("workload = \"api\"", "workload = \"missing\""),
        DEFINITION.replace("${endpoints.public-api.url}", "${endpoints.missing.url}"),
        DEFINITION.replace(
            "[workloads.api]\nrun = \"server\"\nhealth = { path = \"/\" }",
            "[workloads.api]\nkind = 'worker'\nrun = 'sleep 1'",
        ),
    ] {
        assert!(
            StackDef::parse(&invalid)
                .unwrap()
                .validate_hosts(&["local"])
                .is_err()
        );
    }
}
