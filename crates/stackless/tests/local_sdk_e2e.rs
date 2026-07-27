//! Hermetic local SDK e2e: embedded daemon + hello fixture.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::time::Duration;

use stackless::client::Create;
use stackless::test_support::{GuardPolicy, TestContext};

#[path = "support/hello_stack_bind.rs"]
mod stack_bind;

#[test]
fn hello_fixture_up_http_down() {
    let ctx = TestContext::new().expect("test context");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/hello");
    let create = Create::new(fixture.join("stackless.toml"), "local")
        .named("sdk-hello")
        .source(format!("web={}", fixture.display()));
    let env = ctx
        .environment(create, GuardPolicy::DownOnDrop)
        .expect("up");
    let origins = stack_bind::Origins::from_map(env.origins()).expect("typed origins");
    let body = http_get(&origins.web).expect("http get");
    assert!(
        body.contains("hello-fixture"),
        "expected hello-fixture in body, got:\n{body}"
    );
}

fn http_get(origin: &str) -> Result<String, String> {
    let without_scheme = origin
        .strip_prefix("http://")
        .ok_or_else(|| format!("unsupported origin {origin}"))?;
    let (host, port) = without_scheme
        .rsplit_once(':')
        .ok_or_else(|| format!("origin missing port: {origin}"))?;
    let port: u16 = port
        .parse()
        .map_err(|err| format!("bad port in {origin}: {err}"))?;

    let mut last = String::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        match try_get(host, port) {
            Ok(body) if body.contains("hello-fixture") => return Ok(body),
            Ok(body) => last = body,
            Err(err) => last = err,
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err(last)
}

fn try_get(host: &str, port: u16) -> Result<String, String> {
    let mut stream = TcpStream::connect(("127.0.0.1", port))
        .map_err(|err| format!("connect 127.0.0.1:{port}: {err}"))?;
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok();
    stream.set_write_timeout(Some(Duration::from_secs(2))).ok();
    let request = format!("GET / HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .map_err(|err| format!("write: {err}"))?;
    let mut buf = String::new();
    stream
        .read_to_string(&mut buf)
        .map_err(|err| format!("read: {err}"))?;
    Ok(buf)
}
