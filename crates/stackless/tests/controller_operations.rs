//! The caller can exit. The controller owns the operation and its processes.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::Write;
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use stackless::client::{OperationStatus, UpOutcome};
use stackless::{Client, Create, UpRequest};
use stackless_core::paths::Paths;
use stackless_core::state::Store;
use stackless_core::types::TcpPort;
use stackless_daemon::{DaemonClient, rpc::Request};

struct Fixture {
    root: tempfile::TempDir,
    client: Client,
    daemon: Option<Child>,
    port: TcpPort,
}

impl Fixture {
    fn remote_request(&self, request: serde_json::Value) -> serde_json::Value {
        let bridge = self.root.path().join("bridge");
        std::fs::create_dir_all(&bridge).unwrap();
        if !bridge.join("stackless").exists() {
            std::os::unix::fs::symlink(self.client.paths().state_dir(), bridge.join("stackless"))
                .unwrap();
        }
        let mut child = Command::new(env!("CARGO_BIN_EXE_stackless"))
            .args(["daemon", "remote-control"])
            .env("XDG_STATE_HOME", bridge)
            .env("STACKLESS_NO_SELF_UPDATE", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let bytes =
            serde_json::to_vec(&serde_json::json!({"protocol": 2, "request": request})).unwrap();
        child.stdin.take().unwrap().write_all(&bytes).unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "bridge failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let wire: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(wire["protocol"], 2);
        wire["reply"].clone()
    }

    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let state = root.path().join("state");
        let socket = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = TcpPort::from_os(socket.local_addr().unwrap().port());
        drop(socket);
        let client = Client::builder()
            .paths(Paths::new(state))
            .proxy_port(port)
            .build()
            .unwrap();
        let mut fixture = Self {
            root,
            client,
            daemon: None,
            port,
        };
        fixture.start_daemon();
        fixture
    }

    fn start_daemon(&mut self) {
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.root.path().join("controller.log"))
            .unwrap();
        let child = Command::new(env!("CARGO_BIN_EXE_stackless"))
            .args(["daemon", "run", "--embedded", "--state-dir"])
            .arg(self.client.paths().state_dir())
            .arg("--proxy-port")
            .arg(self.port.get().to_string())
            .current_dir(self.root.path())
            .env("STACKLESS_NO_SELF_UPDATE", "1")
            .env("STACKLESS_TEST_OPERATOR_SECRET", "controller-only-canary")
            .env_remove("STACKLESS_STATE_URL")
            .env_remove("STACKLESS_STATE_TOKEN")
            .stdin(Stdio::null())
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .spawn()
            .unwrap();
        self.daemon = Some(child);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(mut daemon) = DaemonClient::connect_with(self.client.paths())
                && daemon.ping().is_ok()
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "controller did not start: {}",
                std::fs::read_to_string(self.root.path().join("controller.log")).unwrap()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn app(&self, prepare: Option<&str>, delayed_health: bool) -> PathBuf {
        let app = self.root.path().join("app");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::write(
            app.join("server.py"),
            r#"
import http.server, os, pathlib
class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200 if pathlib.Path('ready').exists() else 503)
        self.end_headers()
        self.wfile.write(b'hello-fixture')
http.server.HTTPServer(('127.0.0.1', int(os.environ['PORT'])), Handler).serve_forever()
"#,
        )
        .unwrap();
        if !delayed_health {
            std::fs::write(app.join("ready"), "ready").unwrap();
        }
        let hook = prepare
            .map(|command| format!("prepare = {}\n", serde_json::to_string(command).unwrap()))
            .unwrap_or_default();
        std::fs::write(
            app.join("stackless.toml"),
            format!(
                r#"
[stack]
name = "controller-test"
[services.web]
source = {{ repo = "https://example.invalid/web", ref = "main" }}
health = {{ path = "/", contains = "hello-fixture" }}
{hook}
[services.web.local]
run = "python3 server.py"
"#
            ),
        )
        .unwrap();
        std::fs::canonicalize(app).unwrap()
    }

    fn create(&self, app: &std::path::Path) -> Create {
        Create::new(app.join("stackless.toml"), "local")
            .allow_host_execution()
            .named("demo")
            .source(format!("web={}", app.display()))
    }

    fn wait_started(&self, id: &str, step: &str) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let page = self.client.operation(id, 0).unwrap();
            if page
                .events
                .iter()
                .any(|event| event.event["step_id"] == step && event.event["event"] == "Started")
            {
                break;
            }
            assert!(
                !page.operation.status.terminal(),
                "operation ended early: {:?}",
                page.operation
            );
            assert!(Instant::now() < deadline, "step {step} did not start");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn wait_verification(
        &self,
        id: &str,
        launches: &std::path::Path,
    ) -> (
        stackless_core::state::ResourceRecord,
        stackless_core::durable_command::CommandStamp,
        PathBuf,
    ) {
        let store = Store::open(&self.client.paths().db_path()).unwrap();
        let owner = store.instance("demo").unwrap().unwrap().instance_id;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            for record in store.resources(&owner).unwrap() {
                if record.resource_kind != "verification-command" {
                    continue;
                }
                let receipt: serde_json::Value = serde_json::from_str(&record.payload).unwrap();
                if receipt["operation"] == id
                    && !receipt["command"].is_null()
                    && std::fs::read(launches).ok().as_deref() == Some(b"x")
                {
                    let stamp = serde_json::from_value(receipt["command"].clone()).unwrap();
                    let directory = self
                        .client
                        .paths()
                        .state_dir()
                        .canonicalize()
                        .unwrap()
                        .join("verification")
                        .join(&owner)
                        .join(record.key.strip_prefix("verify:").unwrap());
                    return (record, stamp, directory);
                }
            }
            assert!(
                Instant::now() < deadline,
                "verification did not launch: {:?}",
                self.client.operation(id, 0).unwrap().operation
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

#[test]
fn verification_recovers_after_controller_sigkill_and_cancellation_stops_its_command() {
    let mut fixture = Fixture::new();
    let app = fixture.app(None, false);
    let file = app.join("stackless.toml");
    let definition = std::fs::read_to_string(&file).unwrap();
    std::fs::write(&file, format!(r#"{definition}
[stack.verify]
run = "printf x >> verify-launches; printf 'verification:%s\\n' \"$APP_KEY\"; while [ ! -f finish ]; do sleep 0.1; done; touch verified"
env = {{ APP_KEY = "verify-controller-secret" }}
timeout_secs = 15
[stack.verify.tiers.cancel]
run = "printf x >> cancel-launches; sleep 30; touch cancelled-late"
timeout_secs = 15
"#)).unwrap();
    fixture
        .client
        .up(UpRequest::Create(fixture.create(&app)))
        .unwrap();
    let operation = fixture.client.submit_verify("demo", None).unwrap();
    let (record, command, directory) =
        fixture.wait_verification(&operation.id, &app.join("verify-launches"));
    assert!(command.process().is_alive());
    let mut daemon = fixture.daemon.take().unwrap();
    daemon.kill().unwrap();
    daemon.wait().unwrap();
    std::fs::write(app.join("finish"), "done").unwrap();
    fixture.start_daemon();
    let outcome: stackless::client::VerifyOutcome =
        fixture.client.wait_operation(&operation.id, None).unwrap();
    assert_eq!(outcome.exit_status, 0);
    assert_eq!(PathBuf::from(&outcome.log_path), directory.join("output"));
    assert_eq!(std::fs::read(app.join("verify-launches")).unwrap(), b"x");
    assert!(app.join("verified").exists());
    assert!(command.is_stopped());
    let logs = serde_json::to_string(&fixture.client.logs("demo", None, 20).unwrap()).unwrap();
    assert!(logs.contains("verification:[redacted]"), "{logs}");
    assert!(!logs.contains("verify-controller-secret"));
    let store = Store::open(&fixture.client.paths().db_path()).unwrap();
    assert_eq!(
        store
            .resource(&record.owner_id, &record.key)
            .unwrap()
            .unwrap()
            .resource_id,
        record.resource_id
    );
    drop(store);
    let cancelled = fixture
        .client
        .submit_verify("demo", Some("cancel"))
        .unwrap();
    let (_, command, cancel_directory) =
        fixture.wait_verification(&cancelled.id, &app.join("cancel-launches"));
    fixture.client.cancel_operation(&cancelled.id).unwrap();
    assert!(
        fixture
            .client
            .wait_operation::<stackless::client::VerifyOutcome>(&cancelled.id, None)
            .is_err()
    );
    assert_eq!(
        fixture
            .client
            .operation(&cancelled.id, 0)
            .unwrap()
            .operation
            .status,
        OperationStatus::Cancelled
    );
    assert!(command.is_stopped());
    assert!(!app.join("cancelled-late").exists());
    std::fs::remove_file(app.join("cancel-launches")).unwrap();
    let interrupted_cancel = fixture
        .client
        .submit_verify("demo", Some("cancel"))
        .unwrap();
    let (_, command, interrupted_directory) =
        fixture.wait_verification(&interrupted_cancel.id, &app.join("cancel-launches"));
    let mut daemon = fixture.daemon.take().unwrap();
    // Freeze the worker before persisting cancellation, so restart must perform the stop.
    let stopped = Command::new("/bin/kill")
        .args(["-STOP", &daemon.id().to_string()])
        .status();
    let cancellation = Store::open(&fixture.client.paths().db_path())
        .and_then(|store| store.cancel_operation(&interrupted_cancel.id));
    let killed = daemon.kill();
    let waited = daemon.wait();
    assert!(stopped.unwrap().success());
    assert!(cancellation.unwrap());
    killed.unwrap();
    waited.unwrap();
    assert!(command.process().is_alive());
    fixture.start_daemon();
    assert!(
        fixture
            .client
            .wait_operation::<stackless::client::VerifyOutcome>(&interrupted_cancel.id, None)
            .is_err()
    );
    assert_eq!(
        fixture
            .client
            .operation(&interrupted_cancel.id, 0)
            .unwrap()
            .operation
            .status,
        OperationStatus::Cancelled
    );
    assert!(command.is_stopped());
    assert!(!app.join("cancelled-late").exists());
    fixture.client.down("demo").unwrap();
    assert!(!directory.exists());
    assert!(!cancel_directory.exists());
    assert!(!interrupted_directory.exists());
}

#[test]
fn verification_deadline_survives_controller_death_and_keeps_failed_output() {
    let mut fixture = Fixture::new();
    let app = fixture.app(None, false);
    let file = app.join("stackless.toml");
    let definition = std::fs::read_to_string(&file).unwrap();
    std::fs::write(&file, format!(r#"{definition}
[stack.verify]
run = "printf x >> verify-launches; printf 'deadline:%s\\n' \"$APP_KEY\"; sleep 30; touch late-verification"
env = {{ APP_KEY = "verify-deadline-secret" }}
timeout_secs = 3
"#)).unwrap();
    fixture
        .client
        .up(UpRequest::Create(fixture.create(&app)))
        .unwrap();
    let operation = fixture.client.submit_verify("demo", None).unwrap();
    let (_, command, directory) =
        fixture.wait_verification(&operation.id, &app.join("verify-launches"));
    let mut daemon = fixture.daemon.take().unwrap();
    daemon.kill().unwrap();
    daemon.wait().unwrap();
    let deadline = Instant::now() + Duration::from_secs(6);
    while !command.is_stopped() {
        assert!(
            Instant::now() < deadline,
            "verification outlived its watchdog"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        stackless_core::durable_command::outcome(&directory.join("exit"))
            .unwrap()
            .unwrap()
            .cause,
        stackless_core::durable_command::ExitCause::Timeout
    );
    fixture.start_daemon();
    assert!(
        fixture
            .client
            .wait_operation::<stackless::client::VerifyOutcome>(&operation.id, None)
            .is_err()
    );
    let page = fixture.client.operation(&operation.id, 0).unwrap();
    let error = serde_json::to_string(&page.operation.error).unwrap();
    assert!(error.contains("verify.timeout"), "{error}");
    assert!(error.contains("[redacted]"), "{error}");
    assert!(!error.contains("verify-deadline-secret"));
    assert_eq!(std::fs::read(app.join("verify-launches")).unwrap(), b"x");
    assert!(!app.join("late-verification").exists());
    let logs = serde_json::to_string(&fixture.client.logs("demo", None, 20).unwrap()).unwrap();
    assert!(logs.contains("deadline:[redacted]"), "{logs}");
    fixture.client.down("demo").unwrap();
    assert!(!directory.exists());
}

#[test]
fn remote_upload_runs_without_the_callers_files_and_is_owned_until_teardown() {
    let mut fixture = Fixture::new();
    let app = fixture.app(None, false);
    std::fs::write(
        app.join(".stackless.env"),
        "SHOULD_NOT_UPLOAD=credential-canary\n",
    )
    .unwrap();
    let archive = stackless_core::source_archive::SourceArchive::capture(&app).unwrap();
    let id = stackless_core::state::new_operation_id();
    let request = serde_json::json!({"method": "submit_remote_up", "id": id,
        "args": {"name": "remote-demo", "on": "local", "file": null, "sources": [], "dirty": false, "allow_host_execution": true, "lease": null, "confirm_paid": false},
        "definition": std::fs::read_to_string(app.join("stackless.toml")).unwrap(), "sources": {"web": archive}});
    let accepted = fixture.remote_request(request.clone());
    assert!(accepted.get("error").is_none(), "{accepted}");
    assert_eq!(accepted["value"]["id"], id);
    std::fs::remove_dir_all(&app).unwrap();
    let outcome: UpOutcome = fixture.client.wait_operation(&id, None).unwrap();
    assert_eq!(outcome.name, "remote-demo");
    let store = Store::open(&fixture.client.paths().db_path()).unwrap();
    let owner = store.instance("remote-demo").unwrap().unwrap();
    let input = store
        .resources(&owner.instance_id)
        .unwrap()
        .into_iter()
        .find(|resource| resource.resource_kind == "controller-submission")
        .unwrap();
    let uploaded = PathBuf::from(&input.resource_id);
    assert!(uploaded.join("sources/web/server.py").is_file());
    assert!(!uploaded.join("sources/web/.stackless.env").exists());
    let recorded = store.operation_request(&id).unwrap();
    assert_eq!(recorded["verb"], "remote_up");
    assert!(recorded["sources"]["web"]["files"].is_array());
    fixture.daemon.as_mut().unwrap().kill().unwrap();
    fixture.daemon.as_mut().unwrap().wait().unwrap();
    fixture.daemon = None;
    fixture.start_daemon();
    let again = fixture.remote_request(request.clone());
    assert_eq!(again["value"]["id"], id);
    assert_eq!(again["value"]["status"], "succeeded");
    fixture.client.down("remote-demo").unwrap();
    assert!(!uploaded.exists());
    assert!(
        store
            .resources(&owner.instance_id)
            .unwrap()
            .iter()
            .all(|resource| resource.phase == stackless_core::state::ResourcePhase::Absent)
    );
    fixture.daemon.as_mut().unwrap().kill().unwrap();
    fixture.daemon.as_mut().unwrap().wait().unwrap();
    fixture.daemon = None;
    store.delete_instance("remote-demo").unwrap();
    store
        .conn_for_tests()
        .execute(
            "UPDATE operations SET updated_at = ?1 WHERE id = ?2",
            (Store::now_secs() - 8 * 86400, &id),
        )
        .unwrap();
    fixture.start_daemon();
    let deadline = Instant::now() + Duration::from_secs(5);
    while store.operation_request(&id).is_ok() {
        assert!(
            Instant::now() < deadline,
            "controller did not collect expired input"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let again = fixture.remote_request(request.clone());
    assert_eq!(again["value"]["id"], id);
    assert_eq!(again["value"]["status"], "succeeded");
    let mut different = request;
    different["definition"] = serde_json::json!("different definition");
    assert!(fixture.remote_request(different).get("error").is_some());
    assert!(store.instance("remote-demo").unwrap().is_none());
}

#[test]
fn controller_confines_environment_and_redacts_logs_after_restart() {
    let mut fixture = Fixture::new();
    let app = fixture.app(
        Some("test -z \"$STACKLESS_TEST_OPERATOR_SECRET\" && test -n \"$APP_TOKEN\""),
        false,
    );
    let file = app.join("stackless.toml");
    let definition = std::fs::read_to_string(&file).unwrap().replace(
        "[services.web]",
        "[secrets]\nrequired = [\"APP_TOKEN\"]\n[services.web]\nsecrets = [\"APP_TOKEN\"]",
    );
    std::fs::write(&file, format!("{definition}\n[stack.verify]\nrun = \"printf '%s\\n' \\\"$APP_TOKEN\\\"; test -z \\\"$STACKLESS_TEST_OPERATOR_SECRET\\\" && exit 17\"\nenv = {{ APP_TOKEN = \"${{secrets.APP_TOKEN}}\" }}\n")).unwrap();
    std::fs::write(
        app.join(".stackless.env"),
        "APP_TOKEN=application-secret-canary\n",
    )
    .unwrap();
    let server = std::fs::read_to_string(app.join("server.py")).unwrap();
    std::fs::write(app.join("server.py"), format!("import os\nassert 'STACKLESS_TEST_OPERATOR_SECRET' not in os.environ\nassert os.environ['APP_TOKEN'] == 'application-secret-canary'\nprint(os.environ['APP_TOKEN'], flush=True)\n{server}")).unwrap();
    let outcome = fixture
        .client
        .up(UpRequest::Create(fixture.create(&app)))
        .unwrap();
    assert!(!outcome.instance_id.is_empty());
    let log = fixture.client.logs("demo", None, 100).unwrap();
    let text = serde_json::to_string(&log).unwrap();
    assert!(text.contains("[redacted]"), "{text}");
    assert!(!text.contains("application-secret-canary"));
    assert!(!text.contains("controller-only-canary"));
    let error = fixture.client.verify("demo", None).unwrap_err();
    let report = serde_json::to_string(&stackless_core::fault::Report::from_fault(&error)).unwrap();
    assert!(report.contains("[redacted]"), "{report}");
    assert!(!report.contains("application-secret-canary"));
    let history = serde_json::to_string(&fixture.client.operations(Some("demo")).unwrap()).unwrap();
    assert!(!history.contains("application-secret-canary"));
    let mut daemon = fixture.daemon.take().unwrap();
    daemon.kill().unwrap();
    daemon.wait().unwrap();
    std::fs::write(
        app.join(".stackless.env"),
        "APP_TOKEN=rotated-secret-canary\n",
    )
    .unwrap();
    fixture.start_daemon();
    let logs = serde_json::to_string(&fixture.client.logs("demo", None, 100).unwrap()).unwrap();
    assert!(logs.contains("[redacted]"));
    assert!(!logs.contains("application-secret-canary"));
    let operations = fixture.client.operations(Some("demo")).unwrap();
    assert!(
        operations
            .iter()
            .all(|operation| operation.instance_id.as_deref() == Some(&outcome.instance_id))
    );
    assert!(
        !serde_json::to_string(&operations)
            .unwrap()
            .contains("application-secret-canary")
    );
}

#[test]
fn status_rechecks_health_and_resume_replaces_changed_workload_configuration() {
    let fixture = Fixture::new();
    let app = fixture.app(None, false);
    let file = app.join("stackless.toml");
    let definition = std::fs::read_to_string(&file).unwrap().replace(
        "[services.web]",
        "[services.web]\nenv = { VERSION = 'first' }",
    );
    std::fs::write(&file, &definition).unwrap();
    let server = std::fs::read_to_string(app.join("server.py"))
        .unwrap()
        .replace(
            "b'hello-fixture'",
            "('hello-fixture ' + os.environ['VERSION']).encode()",
        );
    std::fs::write(app.join("server.py"), server).unwrap();
    let outcome = fixture
        .client
        .up(UpRequest::Create(fixture.create(&app)))
        .unwrap();
    let store = Store::open(&fixture.client.paths().db_path()).unwrap();
    let first = store.checkpoint("demo", "start:web").unwrap().unwrap();
    let report = fixture.client.status("demo").unwrap();
    assert_eq!(report.services[0].stage, "healthy");
    std::fs::remove_file(app.join("ready")).unwrap();
    let report = fixture.client.status("demo").unwrap();
    assert_eq!(
        report.services[0].observed.readiness,
        stackless::Readiness::Unready
    );
    assert_eq!(report.services[0].stage, "unhealthy");
    std::fs::write(app.join("ready"), "ready").unwrap();
    std::fs::write(&file, definition.replace("'first'", "'second'")).unwrap();
    let updated = fixture
        .client
        .up(UpRequest::Resume(
            stackless::Resume::new("demo").file(&file),
        ))
        .unwrap();
    assert_eq!(outcome.instance_id, updated.instance_id);
    let second = store.checkpoint("demo", "start:web").unwrap().unwrap();
    assert_ne!(first.resource_id, second.resource_id);
    let report = fixture.client.status("demo").unwrap();
    assert_eq!(
        report.services[0].observed.configuration,
        stackless::Configuration::Applied
    );
    assert_eq!(report.services[0].stage, "healthy");
    let snapshot = store.instance("demo").unwrap().unwrap();
    assert!(snapshot.definition.contains("'second'"));
    let start: stackless_core::checkpoint::StartCheckpoint =
        serde_json::from_str(&second.payload).unwrap();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let body = runtime.block_on(async {
        reqwest::get(format!("http://127.0.0.1:{}/", start.port.get()))
            .await
            .unwrap()
            .text()
            .await
            .unwrap()
    });
    assert!(body.ends_with("second"), "{body}");
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if self
            .daemon
            .as_mut()
            .is_some_and(|daemon| daemon.try_wait().ok().flatten().is_none())
        {
            let _ = self.client.down("demo");
        }
        if let Ok(store) = Store::open(&self.client.paths().db_path())
            && let Ok(Some(checkpoint)) = store.checkpoint("demo", "start:web")
            && let Ok(payload) = serde_json::from_str::<stackless_core::checkpoint::StartCheckpoint>(
                &checkpoint.payload,
            )
        {
            let stamp = stackless_core::process::ProcessStamp {
                pid: payload.pid,
                start_time: payload.start_time,
            };
            if stamp.is_alive() {
                stackless_core::process::kill_process_tree(stamp.pid.get());
            }
        }
        if let Ok(mut daemon) = DaemonClient::connect_with(self.client.paths()) {
            let _ = daemon.call(Request::Shutdown);
        }
        if let Some(mut daemon) = self.daemon.take() {
            let deadline = Instant::now() + Duration::from_secs(3);
            while daemon.try_wait().ok().flatten().is_none() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(20));
            }
            let _ = daemon.kill();
            let _ = daemon.wait();
        }
    }
}

#[test]
fn cli_exits_after_submit_and_operation_finishes_in_controller() {
    let fixture = Fixture::new();
    let app = fixture.app(Some("sleep 0.3; printf once >> executions"), false);
    let output = Command::new(env!("CARGO_BIN_EXE_stackless"))
        .args([
            "up",
            "--allow-host-execution",
            "--name",
            "demo",
            "--on",
            "local",
            "--no-wait",
            "--json",
            "--file",
        ])
        .arg(app.join("stackless.toml"))
        .arg("--source")
        .arg(format!("web={}", app.display()))
        .arg("--state-dir")
        .arg(fixture.client.paths().state_dir())
        .arg("--proxy-port")
        .arg(fixture.port.get().to_string())
        .env("STACKLESS_NO_SELF_UPDATE", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let id = value["operation"]["id"].as_str().unwrap();
    let outcome: UpOutcome = fixture.client.wait_operation(id, None).unwrap();
    assert_eq!(outcome.name, "demo");
    assert_eq!(
        std::fs::read_to_string(app.join("executions")).unwrap(),
        "once"
    );
    let repeated = fixture
        .client
        .submit_up_with_id(id, UpRequest::Create(fixture.create(&app)))
        .unwrap();
    assert_eq!(repeated.status, OperationStatus::Succeeded);
    assert_eq!(fixture.client.operations(Some("demo")).unwrap().len(), 1);
}

#[test]
fn cancellation_stops_before_start_and_leaves_resources_available_for_down() {
    let fixture = Fixture::new();
    let app = fixture.app(Some("sleep 0.5"), false);
    let operation = fixture
        .client
        .submit_up(UpRequest::Create(fixture.create(&app)))
        .unwrap();
    fixture.wait_started(&operation.id, "prepare:web");
    assert!(
        fixture
            .client
            .cancel_operation(&operation.id)
            .unwrap()
            .cancel_requested
    );
    assert!(
        fixture
            .client
            .wait_operation::<UpOutcome>(&operation.id, None)
            .is_err()
    );
    let page = fixture.client.operation(&operation.id, 0).unwrap();
    assert_eq!(page.operation.status, OperationStatus::Cancelled);
    assert!(
        !page
            .events
            .iter()
            .any(|event| event.event["step_id"] == "start:web")
    );
    fixture.client.down("demo").unwrap();
}

#[test]
fn controller_sigkill_recovers_same_operation_without_starting_a_second_service() {
    let mut fixture = Fixture::new();
    let app = fixture.app(None, true);
    let operation = fixture
        .client
        .submit_up(UpRequest::Create(fixture.create(&app)))
        .unwrap();
    fixture.wait_started(&operation.id, "health:web");
    let store = Store::open(&fixture.client.paths().db_path()).unwrap();
    let before = store.checkpoint("demo", "start:web").unwrap().unwrap();
    drop(store);
    let mut daemon = fixture.daemon.take().unwrap();
    daemon.kill().unwrap();
    daemon.wait().unwrap();
    std::fs::write(app.join("ready"), "ready").unwrap();
    fixture.start_daemon();
    let outcome: UpOutcome = fixture.client.wait_operation(&operation.id, None).unwrap();
    assert!(outcome.skipped.iter().any(|step| step == "start:web"));
    let store = Store::open(&fixture.client.paths().db_path()).unwrap();
    let after = store.checkpoint("demo", "start:web").unwrap().unwrap();
    assert_eq!(before.resource_id, after.resource_id);
    assert_eq!(before.payload, after.payload);
    assert_eq!(fixture.client.operations(Some("demo")).unwrap().len(), 1);
    let owner = store.instance("demo").unwrap().unwrap().instance_id;
    fixture.client.down("demo").unwrap();
    let reader = Store::open(&fixture.client.paths().db_path()).unwrap();
    assert_eq!(
        reader.instance("demo").unwrap().unwrap().status,
        stackless_core::state::InstanceStatus::Tombstoned
    );
    assert!(
        reader
            .resources(&owner)
            .unwrap()
            .iter()
            .all(|resource| resource.phase == stackless_core::state::ResourcePhase::Absent)
    );
}

#[test]
fn dropped_submit_response_can_be_recovered_with_the_same_id() {
    let fixture = Fixture::new();
    let app = fixture.app(None, false);
    let definition = std::fs::read_to_string(app.join("stackless.toml")).unwrap();
    let id = stackless_core::state::new_operation_id();
    let wire = serde_json::json!({
        "protocol":2, "version":env!("CARGO_PKG_VERSION"), "cmd":"control",
        "request":{"method":"submit", "id":id, "command":{
            "verb":"up", "args":{"name":"demo", "file":app.join("stackless.toml"), "on":"local", "sources":[format!("web={}",app.display())], "dirty":false, "allow_host_execution":true, "lease":null, "confirm_paid":false},
            "definition":definition, "cwd":std::env::current_dir().unwrap()
        }}
    });
    let mut socket =
        std::os::unix::net::UnixStream::connect(fixture.client.paths().socket_path()).unwrap();
    writeln!(socket, "{wire}").unwrap();
    drop(socket);
    let accepted = fixture
        .client
        .submit_up_with_id(&id, UpRequest::Create(fixture.create(&app)))
        .unwrap();
    let _: UpOutcome = fixture.client.wait_operation(&accepted.id, None).unwrap();
    let events = fixture.client.operation(&id, 0).unwrap().events;
    assert_eq!(
        events
            .iter()
            .filter(
                |event| event.event["step_id"] == "start:web" && event.event["event"] == "Started"
            )
            .count(),
        1
    );
}

#[test]
fn jobs_and_hooks_recover_after_controller_sigkill_without_duplicate_execution() {
    let mut fixture = Fixture::new();
    let app = fixture.root.path().join("jobs-app");
    std::fs::create_dir_all(&app).unwrap();
    let source = serde_json::to_string(&app.display().to_string()).unwrap();
    let file = app.join("stackless.toml");
    std::fs::write(
        &file,
        format!(
            r#"
[stack]
name = "job-test"
[jobs.migrate]
source = {{ path = {source} }}
setup = "printf s >> setup-count"
prepare = "printf p >> prepare-count"
run = "printf j >> job-count; while ! test -f release; do sleep 0.05; done; touch migrated"
timeout_secs = 20
[workloads.worker]
kind = "worker"
source = {{ path = {source} }}
run = "test -f migrated && sleep 300"
depends_on = {{ migrate = "completed" }}
"#
        ),
    )
    .unwrap();
    let operation = fixture
        .client
        .submit_up(UpRequest::Create(
            Create::new(&file, "local")
                .named("demo")
                .allow_host_execution(),
        ))
        .unwrap();
    fixture.wait_started(&operation.id, "job:migrate");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !app.join("job-count").exists() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    let store = Store::open(&fixture.client.paths().db_path()).unwrap();
    let owner = store.instance("demo").unwrap().unwrap().instance_id;
    let before = store
        .resources(&owner)
        .unwrap()
        .into_iter()
        .find(|resource| resource.step_id == "job:migrate")
        .unwrap();
    drop(store);
    let mut daemon = fixture.daemon.take().unwrap();
    daemon.kill().unwrap();
    daemon.wait().unwrap();
    fixture.start_daemon();
    std::fs::write(app.join("release"), "go").unwrap();
    let outcome: UpOutcome = fixture.client.wait_operation(&operation.id, None).unwrap();
    assert!(outcome.origins.is_empty());
    assert_eq!(std::fs::read_to_string(app.join("job-count")).unwrap(), "j");
    assert_eq!(
        std::fs::read_to_string(app.join("prepare-count")).unwrap(),
        "p"
    );
    let report = fixture.client.status("demo").unwrap();
    let job = report
        .services
        .iter()
        .find(|s| s.service == "migrate")
        .unwrap();
    assert_eq!(job.stage, "completed");
    assert!(job.origin.is_none());
    let worker = report
        .services
        .iter()
        .find(|s| s.service == "worker")
        .unwrap();
    assert_eq!(
        worker.observed.readiness,
        stackless::client::Readiness::Ready
    );
    assert!(worker.origin.is_none());
    let store = Store::open(&fixture.client.paths().db_path()).unwrap();
    let after = store.resource(&owner, &before.key).unwrap().unwrap();
    assert_eq!(before.resource_id, after.resource_id);
    drop(store);
    fixture
        .client
        .up(UpRequest::Resume(stackless::Resume::new("demo")))
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(app.join("setup-count")).unwrap(),
        "s"
    );
    assert_eq!(
        std::fs::read_to_string(app.join("prepare-count")).unwrap(),
        "pp"
    );
    assert_eq!(std::fs::read_to_string(app.join("job-count")).unwrap(), "j");
    fixture.client.down("demo").unwrap();
}

#[test]
fn job_timeout_stops_the_process_and_never_starts_dependents() {
    let fixture = Fixture::new();
    let app = fixture.root.path().join("timed-job");
    std::fs::create_dir_all(&app).unwrap();
    let source = serde_json::to_string(&app.display().to_string()).unwrap();
    let file = app.join("stackless.toml");
    std::fs::write(
        &file,
        format!(
            r#"
[stack]
name = "timeout-test"
[jobs.migrate]
source = {{ path = {source} }}
run = "sleep 30"
timeout_secs = 1
[workloads.worker]
kind = "worker"
source = {{ path = {source} }}
run = "touch unexpected; sleep 300"
depends_on = {{ migrate = "completed" }}
"#
        ),
    )
    .unwrap();
    let operation = fixture
        .client
        .submit_up(UpRequest::Create(
            Create::new(&file, "local")
                .named("demo")
                .allow_host_execution(),
        ))
        .unwrap();
    assert!(
        fixture
            .client
            .wait_operation::<UpOutcome>(&operation.id, None)
            .is_err()
    );
    let page = fixture.client.operation(&operation.id, 0).unwrap();
    assert_eq!(page.operation.status, OperationStatus::Failed);
    assert!(!app.join("unexpected").exists());
    let store = Store::open(&fixture.client.paths().db_path()).unwrap();
    let owner = store.instance("demo").unwrap().unwrap().instance_id;
    let job = store
        .resources(&owner)
        .unwrap()
        .into_iter()
        .find(|resource| resource.step_id == "job:migrate")
        .unwrap();
    let payload: stackless_local::job::JobCheckpoint = serde_json::from_str(&job.payload).unwrap();
    assert!(
        !stackless_core::process::ProcessStamp {
            pid: payload.pid,
            start_time: payload.start_time
        }
        .is_alive()
    );
    drop(store);
    fixture.client.down("demo").unwrap();
}

#[test]
fn job_deadline_survives_controller_sigkill_and_retains_failed_output() {
    let mut fixture = Fixture::new();
    let app = fixture.root.path().join("watchdog-job");
    std::fs::create_dir_all(&app).unwrap();
    let source = serde_json::to_string(&app.display().to_string()).unwrap();
    let file = app.join("stackless.toml");
    std::fs::write(
        &file,
        format!(
            r#"
[stack]
name = "watchdog-test"
[jobs.migrate]
source = {{ path = {source} }}
run = "printf 'job-output:%s\\n' \"$APP_KEY\"; printf x >> launches; sleep 30; touch late"
env = {{ APP_KEY = "deadline-secret-canary" }}
timeout_secs = 3
[workloads.worker]
kind = "worker"
source = {{ path = {source} }}
run = "touch unexpected; sleep 300"
depends_on = {{ migrate = "completed" }}
"#
        ),
    )
    .unwrap();
    let operation = fixture
        .client
        .submit_up(UpRequest::Create(
            Create::new(&file, "local")
                .named("demo")
                .allow_host_execution(),
        ))
        .unwrap();
    fixture.wait_started(&operation.id, "job:migrate");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !app.join("launches").exists() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    let store = Store::open(&fixture.client.paths().db_path()).unwrap();
    let owner = store.instance("demo").unwrap().unwrap().instance_id;
    let record = store
        .resources(&owner)
        .unwrap()
        .into_iter()
        .find(|r| r.step_id == "job:migrate")
        .unwrap();
    let payload: stackless_local::job::JobCheckpoint =
        serde_json::from_str(&record.payload).unwrap();
    let command = payload
        .execution
        .as_ref()
        .unwrap()
        .command
        .as_ref()
        .unwrap();
    assert!(command.process().is_alive());
    drop(store);
    let mut daemon = fixture.daemon.take().unwrap();
    daemon.kill().unwrap();
    daemon.wait().unwrap();
    // The controller stays dead until the runner has enforced the deadline.
    let deadline = Instant::now() + Duration::from_secs(6);
    while !command.is_stopped() {
        assert!(
            Instant::now() < deadline,
            "job survived its independent watchdog"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        stackless_core::durable_command::outcome(&payload.result_path)
            .unwrap()
            .unwrap()
            .cause,
        stackless_core::durable_command::ExitCause::Timeout
    );
    assert!(!app.join("late").exists());
    assert!(!app.join("unexpected").exists());
    fixture.start_daemon();
    assert!(
        fixture
            .client
            .wait_operation::<UpOutcome>(&operation.id, None)
            .is_err()
    );
    let page = fixture.client.operation(&operation.id, 0).unwrap();
    assert_eq!(page.operation.status, OperationStatus::Failed);
    assert!(
        serde_json::to_string(&page.operation.error)
            .unwrap()
            .contains("job.timeout")
    );
    assert_eq!(std::fs::read(app.join("launches")).unwrap(), b"x");
    let store = Store::open(&fixture.client.paths().db_path()).unwrap();
    assert_eq!(
        store
            .resource(&owner, &record.key)
            .unwrap()
            .unwrap()
            .resource_id,
        record.resource_id
    );
    assert!(store.checkpoint("demo", "job:migrate").unwrap().is_none());
    drop(store);
    let logs =
        serde_json::to_string(&fixture.client.logs("demo", Some("migrate"), 10).unwrap()).unwrap();
    assert!(
        logs.contains("job-output:") && logs.contains("[redacted]"),
        "{logs}"
    );
    assert!(!logs.contains("deadline-secret-canary"));
    fixture.client.down("demo").unwrap();
    assert!(!payload.result_path.parent().unwrap().exists());
}

#[test]
fn host_execution_requires_caller_grant_and_a_reused_name_does_not_inherit_it() {
    use stackless_core::fault::Fault;
    let fixture = Fixture::new();
    let app = fixture.app(Some("touch executed"), false);
    let request = || {
        UpRequest::Create(
            Create::new(app.join("stackless.toml"), "local")
                .named("demo")
                .source(format!("web={}", app.display())),
        )
    };
    let error = fixture.client.up(request()).unwrap_err();
    assert_eq!(error.code(), "execution.host_grant_required");
    assert!(!app.join("executed").exists());
    let store = Store::open(&fixture.client.paths().db_path()).unwrap();
    assert!(store.instance("demo").unwrap().is_none());
    drop(store);
    fixture
        .client
        .up(UpRequest::Create(fixture.create(&app)))
        .unwrap();
    assert!(app.join("executed").exists());
    fixture.client.down("demo").unwrap();
    let error = fixture.client.up(request()).unwrap_err();
    assert_eq!(error.code(), "execution.host_grant_required");
}

#[test]
#[ignore = "requires a local Docker engine and cached public test images"]
fn container_workloads_enforce_isolation_and_restore_ingress_after_controller_restart() {
    let mut fixture = Fixture::new();
    let app = fixture.root.path().join("container-app");
    std::fs::create_dir_all(app.join(".projects")).unwrap();
    std::fs::write(
        app.join(".stackless.env"),
        "OPERATOR_PRIVATE=not-for-the-workload\n",
    )
    .unwrap();
    std::fs::write(app.join(".projects/token"), "not-for-the-workload").unwrap();
    let operator_file = fixture.root.path().join("outside-secret");
    std::fs::write(&operator_file, "not-for-the-workload").unwrap();
    let source = serde_json::to_string(&app.display().to_string()).unwrap();
    let audit = format!(
        r#"set -eux
 test "$(id -u)" != 0
 test ! -e .stackless.env
 test ! -e .projects
 test ! -e /var/run/docker.sock
 test ! -e '{}'
 test -z "${{STACKLESS_TEST_OPERATOR_SECRET:-}}"
 test -z "${{OPERATOR_PRIVATE:-}}"
 grep -q 'CapEff:.*0000000000000000' /proc/self/status
 grep -q 'NoNewPrivs:.*1' /proc/self/status
 ! touch /etc/stackless-test 2>/dev/null
 ! nc -z -w 1 1.1.1.1 443
 ! ip route | grep -q '^default '
 touch sandbox-only
 printf 'isolated\n'
"#,
        operator_file.display()
    );
    let audit = serde_json::to_string(&audit).unwrap();
    let file = app.join("stackless.toml");
    std::fs::write(&file, format!(r#"
[stack]
name = "container-test"
[jobs.audit]
image = "alpine@sha256:48b0309ca019d89d40f670aa1bc06e426dc0931948452e8491e3d65087abc07d"
source = {{ path = {source} }}
run = {audit}
timeout_secs = 15
[workloads.web]
image = "alpine@sha256:48b0309ca019d89d40f670aa1bc06e426dc0931948452e8491e3d65087abc07d"
source = {{ path = {source} }}
health = {{ path = "/", contains = "hello-fixture" }}
run = "while true; do printf 'HTTP/1.1 200 OK\\r\\nContent-Length:13\\r\\n\\r\\nhello-fixture' | nc -l -p $PORT; done"
depends_on = {{ audit = "completed" }}
timeout_secs = 15
"#)).unwrap();
    let outcome = fixture
        .client
        .up(UpRequest::Create(Create::new(&file, "local").named("demo")))
        .unwrap();
    assert!(
        !app.join("sandbox-only").exists(),
        "container writes escaped into the caller checkout"
    );
    assert!(outcome.origins.contains_key("web"));
    let store = Store::open(&fixture.client.paths().db_path()).unwrap();
    let record = store.instance("demo").unwrap().unwrap();
    assert!(!store.host_execution_allowed(&record.instance_id).unwrap());
    let before = store.checkpoint("demo", "start:web").unwrap().unwrap();
    let payload: stackless_local::workload::DockerCheckpoint =
        serde_json::from_str(&before.payload).unwrap();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let docker = stackless_local::workload::connect().unwrap();
    let inspect = runtime
        .block_on(docker.inspect_container(
            payload.container_id.as_deref().unwrap(),
            None::<bollard::query_parameters::InspectContainerOptions>,
        ))
        .unwrap();
    let config = inspect.host_config.unwrap();
    assert_eq!(config.readonly_rootfs, Some(true));
    assert_eq!(config.privileged, Some(false));
    assert_eq!(config.memory, Some(512 * 1024 * 1024));
    assert_eq!(config.pids_limit, Some(128));
    assert_eq!(config.cap_drop, Some(vec!["ALL".into()]));
    drop(store);
    let mut daemon = fixture.daemon.take().unwrap();
    daemon.kill().unwrap();
    daemon.wait().unwrap();
    fixture.start_daemon();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let report = fixture.client.status("demo").unwrap();
        if report
            .services
            .iter()
            .any(|service| service.service == "web" && service.stage == "healthy")
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "container ingress was not restored"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let store = Store::open(&fixture.client.paths().db_path()).unwrap();
    assert_eq!(
        before.resource_id,
        store
            .checkpoint("demo", "start:web")
            .unwrap()
            .unwrap()
            .resource_id
    );
    drop(store);
    let down = fixture.client.down("demo").unwrap();
    let view = fixture.client.status("demo").unwrap();
    let store = Store::open(&fixture.client.paths().db_path()).unwrap();
    let resources = store.resources(&record.instance_id).unwrap();
    assert!(
        resources
            .iter()
            .all(|resource| resource.phase == stackless_core::state::ResourcePhase::Absent),
        "remaining resource phases: {:?}; instance: {:?}; down: {:?}; controller: {:?}",
        resources
            .iter()
            .map(|resource| (
                &resource.key,
                &resource.step_id,
                &resource.resource_kind,
                resource.phase
            ))
            .collect::<Vec<_>>(),
        store
            .instance("demo")
            .unwrap()
            .map(|instance| (instance.instance_id, instance.status)),
        down,
        view
    );
}

#[test]
#[ignore = "requires a local Docker engine and cached public test images"]
fn container_jobs_recover_running_execution_and_preserve_image_defaults() {
    let mut fixture = Fixture::new();
    let app = fixture.root.path().join("container-jobs");
    std::fs::create_dir_all(&app).unwrap();
    let file = app.join("stackless.toml");
    std::fs::write(
        &file,
        r#"
[stack]
name = "container-jobs"
[jobs.migrate]
image = "alpine@sha256:48b0309ca019d89d40f670aa1bc06e426dc0931948452e8491e3d65087abc07d"
source = { path = "." }
setup = "printf s >> setup-count"
prepare = "printf p >> prepare-count"
run = "printf j >> job-count; while ! test -f release; do sleep 0.05; done"
timeout_secs = 30
[jobs.defaults]
image = "alpine@sha256:48b0309ca019d89d40f670aa1bc06e426dc0931948452e8491e3d65087abc07d"
depends_on = { migrate = "completed" }
timeout_secs = 30
"#,
    )
    .unwrap();
    let operation = fixture
        .client
        .submit_up(UpRequest::Create(Create::new(&file, "local").named("demo")))
        .unwrap();
    fixture.wait_started(&operation.id, "job:migrate");
    let store = Store::open(&fixture.client.paths().db_path()).unwrap();
    let owner = store.instance("demo").unwrap().unwrap().instance_id;
    let materialized = store
        .checkpoint("demo", "materialize:migrate")
        .unwrap()
        .unwrap();
    let source: serde_json::Value = serde_json::from_str(&materialized.payload).unwrap();
    let workspace = PathBuf::from(source["path"].as_str().unwrap());
    let deadline = Instant::now() + Duration::from_secs(10);
    while !workspace.join("job-count").exists() {
        assert!(Instant::now() < deadline, "container job did not start");
        std::thread::sleep(Duration::from_millis(20));
    }
    let before = store
        .resources(&owner)
        .unwrap()
        .into_iter()
        .find(|resource| resource.step_id == "job:migrate")
        .unwrap();
    drop(store);
    let mut daemon = fixture.daemon.take().unwrap();
    daemon.kill().unwrap();
    daemon.wait().unwrap();
    fixture.start_daemon();
    std::fs::write(workspace.join("release"), "go").unwrap();
    let outcome: UpOutcome = fixture.client.wait_operation(&operation.id, None).unwrap();
    assert!(outcome.origins.is_empty());
    for (file, expected) in [
        ("setup-count", "s"),
        ("prepare-count", "p"),
        ("job-count", "j"),
    ] {
        assert_eq!(
            std::fs::read_to_string(workspace.join(file)).unwrap(),
            expected
        );
        assert!(!app.join(file).exists());
    }
    let report = fixture.client.status("demo").unwrap();
    assert_eq!(report.services.len(), 2);
    for job in &report.services {
        assert_eq!(job.stage, "completed");
        assert_eq!(job.exit_code, Some(0));
        assert!(job.origin.is_none());
    }
    let store = Store::open(&fixture.client.paths().db_path()).unwrap();
    assert_eq!(
        before.resource_id,
        store
            .resource(&owner, &before.key)
            .unwrap()
            .unwrap()
            .resource_id
    );
    let defaults = store.checkpoint("demo", "job:defaults").unwrap().unwrap();
    let payload: stackless_local::workload::DockerCheckpoint =
        serde_json::from_str(&defaults.payload).unwrap();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let docker = stackless_local::workload::connect().unwrap();
    let inspect = runtime
        .block_on(docker.inspect_container(
            payload.container_id.as_deref().unwrap(),
            None::<bollard::query_parameters::InspectContainerOptions>,
        ))
        .unwrap();
    assert_eq!(inspect.config.unwrap().cmd, Some(vec!["/bin/sh".into()]));
    assert!(
        inspect
            .mounts
            .unwrap_or_default()
            .iter()
            .all(|mount| mount.destination.as_deref() != Some("/app"))
    );
    drop(store);
    fixture.client.down("demo").unwrap();
    let store = Store::open(&fixture.client.paths().db_path()).unwrap();
    assert!(
        store
            .resources(&owner)
            .unwrap()
            .iter()
            .all(|resource| resource.phase == stackless_core::state::ResourcePhase::Absent)
    );
}

#[test]
fn service_logs_rotate_while_controller_is_dead_and_orphan_cleanup_uses_cookie() {
    let mut fixture = Fixture::new();
    let app = fixture.app(None, false);
    let server = std::fs::read_to_string(app.join("server.py")).unwrap();
    std::fs::write(
        app.join("server.py"),
        format!(
            r#"
import os, pathlib, threading, time
print('short-before-flood', flush=True)
def flood():
    while not pathlib.Path('flood').exists():
        time.sleep(0.01)
    for _ in range(1024):
        os.write(1, b'x' * 8192)
    print('\nretained:' + os.environ['APP_KEY'], flush=True)
    pathlib.Path('flood-done').touch()
threading.Thread(target=flood, daemon=True).start()
{server}
"#
        ),
    )
    .unwrap();
    let definition = app.join("stackless.toml");
    let mut text = std::fs::read_to_string(&definition).unwrap();
    text.push_str("\n[services.web.env]\nAPP_KEY = \"service-log-secret-canary\"\n");
    std::fs::write(&definition, text).unwrap();
    fixture
        .client
        .up(UpRequest::Create(fixture.create(&app)))
        .unwrap();
    let store = Store::open(&fixture.client.paths().db_path()).unwrap();
    let checkpoint = store.checkpoint("demo", "start:web").unwrap().unwrap();
    let receipt: stackless_core::checkpoint::StartCheckpoint =
        serde_json::from_str(&checkpoint.payload).unwrap();
    let command = receipt.command.unwrap();
    let log = PathBuf::from(receipt.log.as_str());
    assert!(
        std::fs::read_to_string(&log)
            .unwrap()
            .contains("short-before-flood")
    );
    let mut daemon = fixture.daemon.take().unwrap();
    daemon.kill().unwrap();
    daemon.wait().unwrap();
    std::fs::write(app.join("flood"), b"").unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if app.join("flood-done").exists()
            && std::fs::read(&log)
                .unwrap_or_default()
                .ends_with(b"retained:service-log-secret-canary\n")
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "service output stopped with the controller"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(command.process().is_alive());
    for index in 0..stackless_local::logging::GENERATIONS {
        let path = if index == 0 {
            log.clone()
        } else {
            log.with_extension(format!("log.{index}"))
        };
        assert!(
            std::fs::metadata(path).unwrap().len() <= stackless_local::logging::GENERATION_BYTES
        );
    }
    assert!(!log.with_extension("log.3").exists());
    fixture.start_daemon();
    let output =
        serde_json::to_string(&fixture.client.logs("demo", Some("web"), 20).unwrap()).unwrap();
    assert!(output.contains("retained:[redacted]"), "{output}");
    assert!(!output.contains("service-log-secret-canary"));
    // Kill only the runner. The service survives in its recorded invocation.
    Command::new("/bin/kill")
        .args(["-KILL", &command.pid.get().to_string()])
        .status()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while command.process().is_alive() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!command.process().is_alive());
    assert!(!command.is_stopped());
    let report = fixture.client.status("demo").unwrap();
    let service = report
        .services
        .iter()
        .find(|service| service.service == "web")
        .unwrap();
    assert_eq!(
        service.observed.configuration,
        stackless::client::Configuration::Drifted
    );
    fixture.client.down("demo").unwrap();
    assert!(command.is_stopped());
}

#[test]
fn worker_with_a_missing_runner_is_not_reported_ready() {
    let fixture = Fixture::new();
    let file = fixture.root.path().join("worker.toml");
    let ready = fixture.root.path().join("worker-ready");
    std::fs::write(&file, format!(r#"[stack]
name='fixture'
[services.worker]
kind='worker'
run = """python3 -c 'import os,pathlib,time; pathlib.Path(os.environ["READY_FILE"]).write_text("ready"); time.sleep(30)'"""
env = {{ READY_FILE = {:?} }}
"#, ready.display().to_string())).unwrap();
    let outcome = fixture
        .client
        .up(UpRequest::Create(
            Create::new(&file, "local")
                .named("demo")
                .allow_host_execution(),
        ))
        .unwrap();
    assert!(outcome.origins.is_empty());
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ready.exists() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }

    let initial = fixture.client.status("demo").unwrap();
    assert_eq!(
        initial.services[0].observed.readiness,
        stackless::Readiness::Ready
    );
    let store = Store::open(&fixture.client.paths().db_path()).unwrap();
    let checkpoint = store.checkpoint("demo", "start:worker").unwrap().unwrap();
    let receipt: stackless_core::checkpoint::StartCheckpoint =
        serde_json::from_str(&checkpoint.payload).unwrap();
    let command = receipt.command.unwrap();
    assert!(
        Command::new("/bin/kill")
            .args(["-KILL", &command.pid.get().to_string()])
            .status()
            .unwrap()
            .success()
    );
    let deadline = Instant::now() + Duration::from_secs(2);
    while command.process().is_alive() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!command.process().is_alive());
    assert!(!command.is_stopped());
    let report = fixture.client.status("demo").unwrap();
    let worker = &report.services[0];
    assert_eq!(
        worker.observed.configuration,
        stackless::Configuration::Drifted
    );
    assert_eq!(worker.observed.readiness, stackless::Readiness::Unknown);
    assert!(worker.origin.is_none());
    assert_eq!(worker.stage, "drifted");
    fixture.client.down("demo").unwrap();
    assert!(command.is_stopped());
}

#[test]
fn named_endpoints_reach_commands_verification_results_and_status() {
    use stackless_core::def::EndpointSource;
    let fixture = Fixture::new();
    let app = fixture.app(None, false);
    let file = app.join("stackless.toml");
    let definition = format!(
        r#"{}
[endpoints.public]
workload = "web"
url = "https://external.example.test/v1"
[endpoints.native]
workload = "web"
[stack.verify]
run = 'test "$PUBLIC" = "https://external.example.test/v1" && test "$NATIVE" = "$ORIGIN"'
env = {{ PUBLIC = "${{endpoints.public.url}}", NATIVE = "${{endpoints.native.url}}", ORIGIN = "${{services.web.origin}}" }}
"#,
        std::fs::read_to_string(&file).unwrap().replace(
            "[services.web]",
            "[services.web]\nenv = { PUBLIC = '${endpoints.public.url}', NATIVE = '${endpoints.native.url}' }"
        )
    );
    std::fs::write(&file, &definition).unwrap();
    let server = std::fs::read_to_string(app.join("server.py"))
        .unwrap()
        .replace(
            "b'hello-fixture'",
            "('hello-fixture ' + os.environ['PUBLIC'] + ' ' + os.environ['NATIVE']).encode()",
        );
    std::fs::write(app.join("server.py"), server).unwrap();
    let first = fixture
        .client
        .up(UpRequest::Create(fixture.create(&app)))
        .unwrap();
    assert_eq!(
        first.endpoint("native").unwrap(),
        first.origin("web").unwrap()
    );
    assert_eq!(
        first.endpoint("public").unwrap(),
        "https://external.example.test/v1"
    );
    assert_eq!(first.endpoints["public"].source, EndpointSource::Declared);
    assert_eq!(first.endpoints["native"].source, EndpointSource::Provider);
    fixture.client.verify("demo", None).unwrap();
    let report = fixture.client.status("demo").unwrap();
    assert_eq!(
        report.endpoints["native"].readiness,
        stackless::Readiness::Ready
    );
    assert_eq!(
        report.endpoints["public"].readiness,
        stackless::Readiness::Unknown
    );
    let store = Store::open(&fixture.client.paths().db_path()).unwrap();
    let first_start = store.checkpoint("demo", "start:web").unwrap().unwrap();
    let check_body = |expected: &str| {
        let checkpoint = store.checkpoint("demo", "start:web").unwrap().unwrap();
        let start: stackless_core::checkpoint::StartCheckpoint =
            serde_json::from_str(&checkpoint.payload).unwrap();
        let body = tokio::runtime::Runtime::new().unwrap().block_on(async {
            reqwest::get(format!("http://127.0.0.1:{}/", start.port.get()))
                .await
                .unwrap()
                .text()
                .await
                .unwrap()
        });
        assert!(body.contains(expected), "{body}");
        assert!(body.ends_with(first.origin("web").unwrap()), "{body}");
    };
    check_body("https://external.example.test/v1");
    let cli = Command::new(env!("CARGO_BIN_EXE_stackless"))
        .args(["up", "--name", "demo", "--json", "--state-dir"])
        .arg(fixture.client.paths().state_dir())
        .arg("--proxy-port")
        .arg(fixture.port.get().to_string())
        .env("STACKLESS_NO_SELF_UPDATE", "1")
        .output()
        .unwrap();
    assert!(
        cli.status.success(),
        "{}",
        String::from_utf8_lossy(&cli.stderr)
    );
    let wire: serde_json::Value = serde_json::from_slice(&cli.stdout).unwrap();
    assert_eq!(wire["endpoints"]["public"]["source"], "declared");
    assert_eq!(
        wire["endpoints"]["native"]["url"],
        first.origin("web").unwrap()
    );
    assert_eq!(
        store
            .checkpoint("demo", "start:web")
            .unwrap()
            .unwrap()
            .resource_id,
        first_start.resource_id
    );
    std::fs::write(
        &file,
        definition.replace("external.example.test", "changed.example.test"),
    )
    .unwrap();
    let updated = fixture
        .client
        .up(UpRequest::Resume(
            stackless::Resume::new("demo").file(&file),
        ))
        .unwrap();
    assert_eq!(
        updated.endpoint("public").unwrap(),
        "https://changed.example.test/v1"
    );
    assert_ne!(
        store
            .checkpoint("demo", "start:web")
            .unwrap()
            .unwrap()
            .resource_id,
        first_start.resource_id
    );
    check_body("https://changed.example.test/v1");
    fixture.client.verify("demo", None).unwrap();
    std::fs::remove_file(app.join("ready")).unwrap();
    let report = fixture.client.status("demo").unwrap();
    assert_eq!(
        report.endpoints["native"].readiness,
        stackless::Readiness::Unready
    );
    assert_eq!(
        report.endpoints["public"].readiness,
        stackless::Readiness::Unknown
    );
    fixture.client.down("demo").unwrap();
    let report = fixture.client.status("demo").unwrap();
    assert!(report.endpoints["native"].url.is_none());
    assert_eq!(report.endpoints["public"].source, EndpointSource::Declared);
    assert_eq!(
        report.endpoints["public"].readiness,
        stackless::Readiness::Unknown
    );
}

#[test]
fn tcp_endpoints_follow_recorded_listeners_across_resume_restart_and_teardown() {
    let mut fixture = Fixture::new();
    let app = fixture.app(None, false);
    let file = app.join("stackless.toml");
    std::fs::write(
        app.join("server.py"),
        r#"
import os, pathlib, socket, time
listener = socket.socket()
listener.bind(('127.0.0.1', int(os.environ['PORT'])))
listener.listen()
listener.settimeout(0.05)
while not pathlib.Path('stop-listener').exists():
    try:
        connection, _ = listener.accept()
    except socket.timeout:
        continue
    with connection:
        try:
            connection.sendall(b'tcp-fixture')
        except OSError:
            pass
listener.close()
pathlib.Path('listener-closed').touch()
while True:
    time.sleep(1)
"#,
    )
    .unwrap();
    let client = "python3 -c \"import os,socket,urllib.parse; u=urllib.parse.urlsplit(os.environ['DB']); assert os.environ['DB']==os.environ['NATIVE']; s=socket.create_connection((u.hostname,u.port),2); assert s.recv(100)==b'tcp-fixture'; print(os.environ['DB'])\"";
    let definition = format!(
        r#"{}
[jobs.client]
run = {}
env = {{ DB = "${{endpoints.database.url}}", NATIVE = "${{services.web.origin}}" }}
depends_on = {{ web = "ready" }}
[endpoints.database]
workload = "web"
[endpoints.external]
workload = "web"
url = "tcp://external.example.test:5432"
[stack.verify]
run = {}
env = {{ DB = "${{endpoints.database.url}}", NATIVE = "${{services.web.origin}}" }}
"#,
        std::fs::read_to_string(&file).unwrap().replace(
            "health = { path = \"/\", contains = \"hello-fixture\" }",
            "health = { protocol = 'tcp' }"
        ),
        serde_json::to_string(client).unwrap(),
        serde_json::to_string(client).unwrap()
    );
    std::fs::write(&file, &definition).unwrap();
    let first = fixture
        .client
        .up(UpRequest::Create(fixture.create(&app)))
        .unwrap();
    let url = first.endpoint("database").unwrap().to_owned();
    assert!(url.starts_with("tcp://127.0.0.1:"), "{url}");
    assert_eq!(first.origin("web").unwrap(), url);
    fixture.client.verify("demo", None).unwrap();
    let store = Store::open(&fixture.client.paths().db_path()).unwrap();
    let start = store.checkpoint("demo", "start:web").unwrap().unwrap();
    let payload: stackless_core::checkpoint::StartCheckpoint =
        serde_json::from_str(&start.payload).unwrap();
    assert!(
        payload.hosts.is_empty(),
        "TCP must not register an HTTP route"
    );
    assert_eq!(url, format!("tcp://127.0.0.1:{}", payload.port.get()));
    let job = store.checkpoint("demo", "job:client").unwrap().unwrap();
    let report = fixture.client.status("demo").unwrap();
    assert_eq!(
        report.endpoints["database"].readiness,
        stackless::Readiness::Ready
    );
    assert_eq!(
        report.endpoints["external"].readiness,
        stackless::Readiness::Unknown
    );
    let resumed = fixture
        .client
        .up(UpRequest::Resume(stackless::Resume::new("demo")))
        .unwrap();
    assert_eq!(resumed.endpoint("database").unwrap(), url);
    assert_eq!(
        store
            .checkpoint("demo", "job:client")
            .unwrap()
            .unwrap()
            .resource_id,
        job.resource_id
    );
    let mut daemon = fixture.daemon.take().unwrap();
    daemon.kill().unwrap();
    daemon.wait().unwrap();
    fixture.start_daemon();
    let report = fixture.client.status("demo").unwrap();
    assert_eq!(
        report.endpoints["database"].url.as_deref(),
        Some(url.as_str())
    );
    assert_eq!(
        report.endpoints["database"].readiness,
        stackless::Readiness::Ready
    );
    std::fs::write(
        &file,
        definition.replace("python3 server.py", "python3 -u server.py"),
    )
    .unwrap();
    let changed = fixture
        .client
        .up(UpRequest::Resume(
            stackless::Resume::new("demo").file(&file),
        ))
        .unwrap();
    let new_start = store.checkpoint("demo", "start:web").unwrap().unwrap();
    assert_ne!(new_start.resource_id, start.resource_id);
    let new_payload: stackless_core::checkpoint::StartCheckpoint =
        serde_json::from_str(&new_start.payload).unwrap();
    assert_eq!(
        changed.endpoint("database").unwrap(),
        format!("tcp://127.0.0.1:{}", new_payload.port.get())
    );
    // A changed URL must invalidate the completed consumer job.
    if new_payload.port != payload.port {
        assert_ne!(
            store
                .checkpoint("demo", "job:client")
                .unwrap()
                .unwrap()
                .resource_id,
            job.resource_id
        );
    }
    fixture.client.verify("demo", None).unwrap();
    std::fs::write(app.join("stop-listener"), "stop").unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !app.join("listener-closed").exists() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }
    let report = fixture.client.status("demo").unwrap();
    assert_eq!(
        report.endpoints["database"].readiness,
        stackless::Readiness::Unready
    );
    fixture.client.down("demo").unwrap();
    let report = fixture.client.status("demo").unwrap();
    assert!(report.endpoints["database"].url.is_none());
    assert!(std::net::TcpStream::connect(("127.0.0.1", new_payload.port.get())).is_err());
}

#[test]
fn cancelled_local_start_does_not_release_journaled_user_code() {
    use stackless_core::substrate::{InstanceContext, StepContext, Substrate};
    // Direct substrate calls need the CLI runner, including on clean CI hosts.
    // Set its path in a child so parallel tests never share environment changes.
    const CHILD: &str = "STACKLESS_TEST_CANCELLED_START_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "cancelled_local_start_does_not_release_journaled_user_code",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env("STACKLESS_BIN", env!("CARGO_BIN_EXE_stackless"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "cancelled start failed: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let fixture = Fixture::new();
    let app = fixture.app(None, false);
    fixture
        .client
        .up(UpRequest::Create(fixture.create(&app)))
        .unwrap();
    let store = Store::open(&fixture.client.paths().db_path()).unwrap();
    let record = store.instance("demo").unwrap().unwrap();
    let checkpoints = store.checkpoints("demo").unwrap();
    let context = InstanceContext::from_record(&record, &checkpoints);
    let def = stackless_core::def::StackDef::parse(
        &record
            .definition
            .replace("python3 server.py", "touch should-not-run; sleep 30"),
    )
    .unwrap();
    let provider = stackless_local::LocalSubstrate {
        state_root: fixture.client.paths().state_dir().to_owned(),
        proxy_port: fixture.port,
        daemon_role: stackless_daemon::DaemonRole::Embedded,
        definition_dir: app.clone(),
        secrets: Default::default(),
    };
    let step = stackless_core::engine::Step {
        id: "start:web".into(),
        node: "web".into(),
        kind: stackless_core::engine::StepKind::Start,
    };
    let error = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(provider.execute(StepContext {
            operation_id: "cancel-before-release",
            store: &store,
            instance: &context,
            def: &def,
            step: &step,
            source_overrides: &record.source_overrides,
            dirty: false,
            prior: &checkpoints,
            parent_resources: &[],
            cancelled: Some(std::sync::Arc::new(std::sync::atomic::AtomicBool::new(
                true,
            ))),
        }))
        .unwrap_err();
    assert_eq!(&*error.code, "operation.cancelled", "{error:?}");
    assert!(!app.join("should-not-run").exists());
    let inventory = store.resources(&record.instance_id).unwrap();
    let processes: Vec<_> = inventory
        .iter()
        .filter(|resource| resource.resource_kind == "process")
        .collect();
    assert_eq!(processes.len(), 2);
    let pending = processes
        .into_iter()
        .find(|resource| {
            resource.resource_id
                != checkpoints
                    .iter()
                    .find(|checkpoint| checkpoint.step_id == "start:web")
                    .unwrap()
                    .resource_id
        })
        .unwrap();
    let payload: stackless_core::checkpoint::StartCheckpoint =
        serde_json::from_str(&pending.payload).unwrap();
    assert!(payload.command.unwrap().is_stopped());
    fixture.client.down("demo").unwrap();
    assert!(
        store
            .resources(&record.instance_id)
            .unwrap()
            .iter()
            .all(|resource| resource.phase == stackless_core::state::ResourcePhase::Absent)
    );
}

#[test]
fn status_reports_unknown_configuration_and_retained_resources_without_payloads() {
    let fixture = Fixture::new();
    let app = fixture.app(None, false);
    fixture
        .client
        .up(UpRequest::Create(fixture.create(&app)))
        .unwrap();
    let store = Store::open(&fixture.client.paths().db_path()).unwrap();
    let original = store.checkpoint("demo", "start:web").unwrap().unwrap();
    // Keep the applied revision but make its process observation fail.
    store
        .conn_for_tests()
        .execute(
            "UPDATE checkpoints SET payload = ?1 WHERE instance = 'demo' AND step_id = 'start:web'",
            ["{\"private_payload\":\"inventory-secret-canary\"}"],
        )
        .unwrap();
    let report = fixture.client.status("demo").unwrap();
    assert_eq!(
        report.services[0].observed.existence,
        stackless::Existence::Unknown
    );
    assert_eq!(
        report.services[0].observed.configuration,
        stackless::Configuration::Unknown
    );
    assert!(report.services[0].observed.error_code.is_some());
    let wire = serde_json::to_string(&report).unwrap();
    assert!(!wire.contains("inventory-secret-canary"));
    store
        .conn_for_tests()
        .execute(
            "UPDATE checkpoints SET payload = ?1 WHERE instance = 'demo' AND step_id = 'start:web'",
            [&original.payload],
        )
        .unwrap();
    store
        .stage_definition(
            "demo",
            "[stack]\nname = 'controller-test'\n[jobs.replacement]\nrun = 'true'",
            "pending-removal",
        )
        .unwrap();
    let report = fixture.client.status("demo").unwrap();
    assert!(
        report
            .services
            .iter()
            .all(|service| service.service != "web")
    );
    let retained = report
        .resources
        .iter()
        .find(|resource| resource.resource_kind == "process")
        .unwrap();
    assert_eq!(retained.step_id, "start:web");
    assert_eq!(retained.on, "local");
    assert_eq!(retained.ownership, stackless_core::state::Ownership::Owned);
    assert!(!retained.desired);
    let value = serde_json::to_value(&report).unwrap();
    assert!(
        value["resources"]
            .as_array()
            .unwrap()
            .iter()
            .all(|resource| resource.get("payload").is_none())
    );
    let legacy = {
        let mut value = value.clone();
        value.as_object_mut().unwrap().remove("resources");
        value
    };
    assert!(
        serde_json::from_value::<stackless::InstanceReport>(legacy)
            .unwrap()
            .resources
            .is_empty()
    );
    fixture.client.down("demo").unwrap();
    assert!(fixture.client.status("demo").unwrap().resources.is_empty());
}
