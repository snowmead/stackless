//! Third-party SDK consumer shape: this test binary is *not* the stackless
//! CLI. Resolving the operator daemon must never return this binary's path.

use stackless_daemon::{DaemonError, ResolveSource, is_cli_process, resolve_daemon_bin};

#[test]
fn test_binary_is_not_marked_as_cli() {
    assert!(
        !is_cli_process(),
        "integration test harness must not call mark_cli_process"
    );
}

#[test]
fn resolve_daemon_bin_never_returns_this_test_binary() {
    let self_exe = std::env::current_exe().expect("current_exe");
    match resolve_daemon_bin() {
        Ok((path, source)) => {
            assert_ne!(
                path, self_exe,
                "operator daemon binary must be the stackless CLI, not the test harness"
            );
            assert_ne!(
                source,
                ResolveSource::SelfAsCli,
                "unmarked consumer must not resolve as SelfAsCli"
            );
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            assert_eq!(
                name,
                "stackless",
                "resolved daemon bin should be named stackless, got {}",
                path.display()
            );
        }
        Err(DaemonError::BinaryNotFound { .. }) => {
            // No CLI installed in this environment — acceptable for CI images
            // that only build the library.
        }
        Err(err) => panic!("unexpected resolve error: {err}"),
    }
}

#[test]
fn client_system_constructs_without_requiring_cli_subcommands() {
    // Constructing Client::system must succeed even when this process is not
    // the CLI; spawn happens later and uses the resolved CLI binary.
    let _client = stackless::Client::system().expect("Client::system constructs");
}
