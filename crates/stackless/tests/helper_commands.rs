//! The real CLI helper owns a Stripe invocation after its calling process exits.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use stackless_core::helper_command::HelperCommand;
use stackless_core::lockfile::FileLock;
use stackless_core::process::{ProcessStamp, TimedCommand};
use stackless_stripe_projects::CommandRunner;

#[test]
fn stripe_caller_entrypoint() {
    let Some(root) = std::env::var_os("STACKLESS_TEST_STRIPE_HELPER_DIR") else {
        return;
    };
    let root = PathBuf::from(root);
    std::fs::write(
        root.join("lock-path"),
        FileLock::stripe_lock_path(&root)
            .as_os_str()
            .as_encoded_bytes(),
    )
    .unwrap();
    let args = ["env", "list", "--json"].map(String::from);
    let _ = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(stackless_stripe_projects::TokioRunner.run(&args, &root));
}

#[test]
fn stripe_command_keeps_its_context_lock_after_caller_sigkill() {
    let root = tempfile::tempdir().unwrap();
    let bin = root.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let stripe = bin.join("stripe");
    std::fs::write(&stripe, r#"#!/usr/bin/env python3
import json, os, pathlib, sys, time
pathlib.Path('ready.tmp').write_text(json.dumps({'pid': os.getpid(), 'helper': os.getppid(), 'args': sys.argv[1:], 'key': os.environ.get('STRIPE_API_KEY'), 'cwd': os.getcwd()}))
os.replace('ready.tmp', 'ready')
while not pathlib.Path('finish').exists(): time.sleep(0.01)
pathlib.Path('done').touch()
print('{"ok":true,"data":{"environments":[]}}', flush=True)
"#).unwrap();
    std::fs::set_permissions(&stripe, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut path = vec![bin];
    path.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    let mut caller = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "stripe_caller_entrypoint", "--nocapture"])
        .env("STACKLESS_TEST_STRIPE_HELPER_DIR", root.path())
        .env("STACKLESS_BIN", env!("CARGO_BIN_EXE_stackless"))
        .env("PATH", std::env::join_paths(path).unwrap())
        .env("XDG_STATE_HOME", root.path().join("xdg"))
        .env("STRIPE_API_KEY", "fake-provider-credential")
        .env("STACKLESS_NO_SELF_UPDATE", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let ready: serde_json::Value = loop {
        if let Ok(bytes) = std::fs::read(root.path().join("ready")) {
            break serde_json::from_slice(&bytes).unwrap();
        }
        assert!(Instant::now() < deadline, "Stripe helper did not launch");
        std::thread::sleep(Duration::from_millis(10));
    };
    let helper = ProcessStamp::of(ready["helper"].as_u64().unwrap() as u32).unwrap();
    let stripe_process = ProcessStamp::of(ready["pid"].as_u64().unwrap() as u32).unwrap();
    let lock = PathBuf::from(std::fs::read_to_string(root.path().join("lock-path")).unwrap());
    assert_eq!(
        ready["args"],
        serde_json::json!(["projects", "env", "list", "--json"])
    );
    assert_eq!(ready["key"], "fake-provider-credential");
    assert_eq!(
        PathBuf::from(ready["cwd"].as_str().unwrap()),
        root.path().canonicalize().unwrap()
    );
    caller.kill().unwrap();
    caller.wait().unwrap();
    assert!(helper.is_alive() && stripe_process.is_alive());
    assert!(FileLock::try_acquire(&lock).is_err());
    let marker = root.path().join("second-command");
    let mut second = HelperCommand::new("/usr/bin/touch");
    second.arg(&marker).lock(&lock, Duration::from_millis(50));
    assert!(matches!(
        second.run_with_cli(
            std::path::Path::new(env!("CARGO_BIN_EXE_stackless")),
            Duration::from_secs(1)
        ),
        TimedCommand::LockFailed { .. }
    ));
    assert!(!marker.exists());
    std::fs::write(root.path().join("finish"), b"").unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while (helper.is_alive() || stripe_process.is_alive()) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    let stopped = !helper.is_alive() && !stripe_process.is_alive();
    stackless_core::process::kill_process_tree_stamped(helper);
    assert!(stopped);
    assert!(root.path().join("done").exists());
    assert!(FileLock::try_acquire(&lock).is_ok());
}
