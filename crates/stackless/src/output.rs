//! Human and `--json` output. JSON goes to stdout, human prose to
//! stderr for errors — agents parse stdout, people read stderr.

use serde::Serialize;

use stackless_core::def::{DependencyGraph, StackDef};
use stackless_core::engine::{ProgressSink, StepProgress, StepProgressEvent, UpOutcome};
use stackless_core::fault::{ErrorContext, Fault, Report};
use stackless_core::substrate::SpendInfo;

const SCHEMA_VERSION: u32 = 1;

struct Capture {
    stdout: std::cell::RefCell<String>,
    stderr: std::cell::RefCell<String>,
}

pub struct Output {
    json: bool,
    capture: Option<Capture>,
}

#[derive(Serialize)]
struct CheckOk<'a> {
    schema_version: u32,
    ok: bool,
    stack: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    substrate: Option<&'a str>,
    services: Vec<&'a str>,
    graph: &'a DependencyGraph,
}

#[derive(Serialize)]
struct ErrorEnvelope {
    ok: bool,
    error: Report,
}

/// `status --json`: the report plus the persistence degradation line so
/// an agent can branch on it (§3).
#[derive(Serialize)]
struct StatusEnvelope<'a> {
    schema_version: u32,
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    persistence_warning: Option<&'a str>,
    #[serde(flatten)]
    report: &'a crate::commands::InstanceStatusReport,
}

/// `list --json`: the same warning alongside the instance array.
#[derive(Serialize)]
struct ListEnvelope<'a> {
    schema_version: u32,
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    persistence_warning: Option<&'a str>,
    instances: &'a [crate::commands::InstanceStatusReport],
}

impl Output {
    pub fn new(json: bool) -> Self {
        Self {
            json,
            capture: None,
        }
    }

    /// Capture stdout/stderr instead of printing — for `stackless mcp`.
    pub fn capturing_json() -> Self {
        Self {
            json: true,
            capture: Some(Capture {
                stdout: std::cell::RefCell::new(String::new()),
                stderr: std::cell::RefCell::new(String::new()),
            }),
        }
    }

    pub fn is_json(&self) -> bool {
        self.json
    }

    pub fn take_capture(self) -> (String, String) {
        match self.capture {
            Some(capture) => (capture.stdout.into_inner(), capture.stderr.into_inner()),
            None => (String::new(), String::new()),
        }
    }

    pub fn check_ok(&self, def: &StackDef, graph: &DependencyGraph, substrate: Option<&str>) {
        if self.json {
            self.emit(&CheckOk {
                schema_version: SCHEMA_VERSION,
                ok: true,
                stack: def.stack.name.as_str(),
                substrate,
                services: def.services.keys().map(String::as_str).collect(),
                graph,
            });
            return;
        }
        println!("stack {:?}: valid", def.stack.name.as_str());
        if let Some(substrate) = substrate {
            println!("  substrate {substrate}: all services configured");
        }
        println!(
            "  services: {}",
            def.services.keys().cloned().collect::<Vec<_>>().join(", ")
        );
        println!(
            "  startup order: {}",
            graph
                .startup_order()
                .iter()
                .map(|node| node.name().to_owned())
                .collect::<Vec<_>>()
                .join(" -> ")
        );
        for (from, to) in graph.wiring() {
            println!("  wiring: {} -> {}", from.name(), to.name());
        }
    }

    pub fn init_ok(&self, path: String, stack: &str) {
        let next = format!("stackless check {path} --on local");
        if self.json {
            #[derive(Serialize)]
            struct InitOk<'a> {
                schema_version: u32,
                ok: bool,
                path: String,
                stack: &'a str,
                next: String,
            }
            self.emit(&InitOk {
                schema_version: SCHEMA_VERSION,
                ok: true,
                path,
                stack,
                next,
            });
            return;
        }
        println!("wrote {path} (stack {stack})");
        println!("next: {next}");
    }

    pub fn adopt_ok(&self, path: String, services: &[&str], merged: bool, next: &str) {
        if self.json {
            #[derive(Serialize)]
            struct AdoptOk<'a> {
                schema_version: u32,
                ok: bool,
                path: String,
                services: &'a [&'a str],
                merged: bool,
                next: &'a str,
            }
            self.emit(&AdoptOk {
                schema_version: SCHEMA_VERSION,
                ok: true,
                path,
                services,
                merged,
                next,
            });
            return;
        }
        let action = if merged { "merged into" } else { "wrote" };
        println!("{action} {path}");
        if !services.is_empty() {
            println!("  services: {}", services.join(", "));
        }
        println!("next: {next}");
    }

    pub fn doctor_ok(&self, all_ok: bool, checks: &[crate::doctor::DoctorCheck]) {
        if self.json {
            #[derive(Serialize)]
            struct DoctorOk<'a> {
                schema_version: u32,
                ok: bool,
                checks: &'a [crate::doctor::DoctorCheck],
            }
            self.emit(&DoctorOk {
                schema_version: SCHEMA_VERSION,
                ok: all_ok,
                checks,
            });
            return;
        }
        for check in checks {
            let mark = if check.ok { "ok" } else { "FAIL" };
            println!("{mark} {}", check.check);
            if let Some(code) = check.code {
                println!("  code: {code}");
            }
            if let Some(remediation) = &check.remediation {
                println!("  remediation: {remediation}");
            }
        }
        if all_ok {
            println!("doctor: all checks passed");
        } else {
            println!("doctor: one or more checks failed");
        }
    }

    pub fn doctor_failed(
        &self,
        checks: &[crate::doctor::DoctorCheck],
        err: &crate::error::CliError,
    ) {
        if self.json {
            #[derive(Serialize)]
            struct DoctorFailed<'a> {
                ok: bool,
                checks: &'a [crate::doctor::DoctorCheck],
                error: Report,
            }
            self.emit(&DoctorFailed {
                ok: false,
                checks,
                error: Report::from_fault(err),
            });
            return;
        }
        self.doctor_ok(false, checks);
        self.fault(err);
    }

    pub fn up_ok(
        &self,
        name: &str,
        substrate: &str,
        outcome: &UpOutcome,
        origins: &[(String, String)],
        spend: Option<&SpendInfo>,
    ) {
        if self.json {
            #[derive(Serialize)]
            struct UpOk<'a> {
                schema_version: u32,
                ok: bool,
                instance: &'a str,
                substrate: &'a str,
                executed: &'a [String],
                skipped: &'a [String],
                duration_ms: u64,
                steps: &'a [stackless_core::engine::StepTiming],
                origins: Vec<Origin<'a>>,
                #[serde(skip_serializing_if = "Option::is_none")]
                spend: Option<&'a SpendInfo>,
            }
            #[derive(Serialize)]
            struct Origin<'a> {
                service: &'a str,
                origin: &'a str,
            }
            self.emit(&UpOk {
                schema_version: SCHEMA_VERSION,
                ok: true,
                instance: name,
                substrate,
                executed: &outcome.executed,
                skipped: &outcome.skipped,
                duration_ms: outcome.duration_ms,
                steps: &outcome.steps,
                origins: origins
                    .iter()
                    .map(|(service, origin)| Origin { service, origin })
                    .collect(),
                spend,
            });
            return;
        }
        println!("{name}: up on {substrate} (all health contracts passed)");
        for (service, origin) in origins {
            println!("  {service}: {origin}");
        }
        if !outcome.skipped.is_empty() {
            println!(
                "  resumed: {} steps already in place",
                outcome.skipped.len()
            );
        }
    }

    pub fn status(
        &self,
        report: &crate::commands::InstanceStatusReport,
        persistence_warning: Option<&str>,
    ) {
        if self.json {
            self.emit(&StatusEnvelope {
                schema_version: SCHEMA_VERSION,
                ok: true,
                persistence_warning,
                report,
            });
            return;
        }
        self.persistence_banner(persistence_warning);
        self.render_report(report);
    }

    /// One instance's human block (shared by `status` and `list`).
    fn render_report(&self, report: &crate::commands::InstanceStatusReport) {
        let lease = report
            .lease_remaining_secs
            .map(|secs| format!("{}m remaining", secs / 60))
            .unwrap_or_else(|| "none".into());
        println!(
            "{} [{}] {} — lease: {}",
            report.name, report.substrate, report.status, lease
        );
        if let Some(reap_failure) = &report.reap_failure {
            println!("  ⚠ {reap_failure}");
        }
        for service in &report.services {
            let alive = match service.alive {
                Some(true) => " (process alive)",
                Some(false) => " (process dead)",
                None => "",
            };
            println!(
                "  {}: {}{} {}",
                service.service, service.stage, alive, service.origin
            );
        }
    }

    /// The loud one-line degradation banner (§3): leases hold only while
    /// the daemon happens to run when persistence is not registered.
    fn persistence_banner(&self, warning: Option<&str>) {
        if let Some(warning) = warning {
            println!("⚠ DEGRADED: {warning}");
        }
    }

    pub fn list(
        &self,
        reports: &[crate::commands::InstanceStatusReport],
        persistence_warning: Option<&str>,
    ) {
        if self.json {
            self.emit(&ListEnvelope {
                schema_version: SCHEMA_VERSION,
                ok: true,
                persistence_warning,
                instances: reports,
            });
            return;
        }
        self.persistence_banner(persistence_warning);
        if reports.is_empty() {
            println!("no instances");
            return;
        }
        for report in reports {
            self.render_report(report);
        }
    }

    /// A line of human progress/debug output (stderr in --json mode so
    /// stdout stays machine-parseable).
    pub fn message(&self, text: &str) {
        if self.json {
            self.write_stderr(text);
        } else {
            println!("{text}");
        }
    }

    pub fn down_ok(&self, name: &str, outcome: &str, spend: Option<&SpendInfo>) {
        if self.json {
            #[derive(Serialize)]
            struct DownOk<'a> {
                schema_version: u32,
                ok: bool,
                instance: &'a str,
                outcome: &'a str,
                #[serde(skip_serializing_if = "Option::is_none")]
                spend: Option<&'a SpendInfo>,
            }
            self.emit(&DownOk {
                schema_version: SCHEMA_VERSION,
                ok: true,
                instance: name,
                outcome,
                spend,
            });
            return;
        }
        match outcome {
            "destroyed" => self.message(&format!(
                "{name}: destroyed, verified gone; tombstone and logs kept"
            )),
            "already_down" => self.message(&format!("{name}: already down")),
            _ => self.message(&format!("{name}: down ({outcome})")),
        }
        if let Some(spend) = spend {
            self.message(&spend.summary);
        }
    }

    pub fn verify_ok(
        &self,
        name: &str,
        tier: Option<&str>,
        duration_ms: u64,
        exit_status: i32,
        log_path: &str,
        lease_remaining_secs: Option<u64>,
    ) {
        if self.json {
            #[derive(Serialize)]
            struct VerifyOk<'a> {
                schema_version: u32,
                ok: bool,
                instance: &'a str,
                #[serde(skip_serializing_if = "Option::is_none")]
                tier: Option<&'a str>,
                duration_ms: u64,
                exit_status: i32,
                log_path: &'a str,
                #[serde(skip_serializing_if = "Option::is_none")]
                lease_remaining_secs: Option<u64>,
            }
            self.emit(&VerifyOk {
                schema_version: SCHEMA_VERSION,
                ok: true,
                instance: name,
                tier,
                duration_ms,
                exit_status,
                log_path,
                lease_remaining_secs,
            });
            return;
        }
        self.message(&format!("{name}: verify passed (lease renewed)"));
    }

    pub fn logs_json(&self, instance: &str, services: &[LogService<'_>]) {
        #[derive(Serialize)]
        struct LogsOk<'a> {
            schema_version: u32,
            ok: bool,
            instance: &'a str,
            services: &'a [LogService<'a>],
        }
        self.emit(&LogsOk {
            schema_version: SCHEMA_VERSION,
            ok: true,
            instance,
            services,
        });
    }

    pub fn logs_unavailable_json(
        &self,
        instance: &str,
        substrate: &str,
        services: &[LogService<'_>],
    ) {
        if self.json {
            #[derive(Serialize)]
            struct LogsUnavailable<'a> {
                schema_version: u32,
                ok: bool,
                instance: &'a str,
                substrate: &'a str,
                services: &'a [LogService<'a>],
            }
            self.emit(&LogsUnavailable {
                schema_version: SCHEMA_VERSION,
                ok: true,
                instance,
                substrate,
                services,
            });
            return;
        }
        self.message(&format!(
            "logs are not retrievable for substrate {substrate:?}"
        ));
    }

    pub fn fault(&self, fault: &dyn Fault) {
        let report = Report::from_fault(fault);
        if self.json {
            self.emit(&ErrorEnvelope {
                ok: false,
                error: report,
            });
            return;
        }
        if let Some(instance) = &report.instance {
            eprintln!("instance: {instance}");
        }
        if let Some(step) = &report.step {
            eprintln!("step: {step}");
        }
        eprintln!("code: {}", report.code);
        eprintln!("message: {}", report.message);
        Self::print_context(&report.context);
        if let Some(tail) = &report.context.log_tail {
            eprintln!("log_tail:");
            eprintln!("{tail}");
        }
        eprintln!("remediation: {}", report.remediation);
    }

    fn print_context(context: &ErrorContext) {
        let field = |label: &str, value: &Option<String>| {
            if let Some(value) = value {
                eprintln!("{label}: {value}");
            }
        };
        field("service", &context.service);
        field("hook", &context.hook);
        field("command", &context.command);
        field("source_dir", &context.source_dir);
        field("log_path", &context.log_path);
        field("log_hint", &context.log_hint);
        field("exit_status", &context.exit_status);
    }

    fn emit<T: Serialize>(&self, value: &T) {
        match serde_json::to_string_pretty(value) {
            Ok(json) => self.write_stdout(&json),
            // Serialization of our own types cannot fail; if it ever
            // does, say so on stderr rather than emitting half-JSON.
            Err(err) => self.write_stderr(&format!("error[cli.json.serialize]: {err}")),
        }
    }

    fn emit_ndjson<T: Serialize>(&self, value: &T) {
        match serde_json::to_string(value) {
            Ok(json) => self.write_stderr(&json),
            Err(err) => self.write_stderr(&format!("error[cli.json.serialize]: {err}")),
        }
    }

    fn write_stdout(&self, text: &str) {
        if let Some(capture) = &self.capture {
            let mut buf = capture.stdout.borrow_mut();
            if !buf.is_empty() {
                buf.push('\n');
            }
            buf.push_str(text);
            return;
        }
        println!("{text}");
    }

    fn write_stderr(&self, text: &str) {
        if let Some(capture) = &self.capture {
            let mut buf = capture.stderr.borrow_mut();
            if !buf.is_empty() {
                buf.push('\n');
            }
            buf.push_str(text);
            return;
        }
        eprintln!("{text}");
    }
}

#[derive(Serialize)]
pub struct LogService<'a> {
    pub service: &'a str,
    pub source: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_path: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lines: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<&'a str>,
}

// `impl ProgressSink for Output` follows this module; reordering is noise.
#[allow(clippy::items_after_test_module)]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logs_json_envelope_shape() {
        let entry = LogService {
            service: "web",
            source: "file",
            log_path: Some("/tmp/demo/web.log".into()),
            lines: vec!["listening on :3000".into()],
            reason: None,
        };
        #[derive(Serialize)]
        struct LogsOk<'a> {
            schema_version: u32,
            ok: bool,
            instance: &'a str,
            services: &'a [LogService<'a>],
        }
        let json = serde_json::to_value(&LogsOk {
            schema_version: SCHEMA_VERSION,
            ok: true,
            instance: "demo",
            services: &[entry],
        })
        .unwrap();
        assert_eq!(json["ok"], true);
        assert_eq!(json["schema_version"], 1);
        assert_eq!(json["instance"], "demo");
        assert_eq!(json["services"][0]["source"], "file");
    }

    #[test]
    fn verify_ok_envelope_shape() {
        #[derive(Serialize)]
        struct VerifyOk {
            schema_version: u32,
            ok: bool,
            instance: &'static str,
            duration_ms: u64,
            exit_status: i32,
            log_path: &'static str,
        }
        let json = serde_json::to_value(&VerifyOk {
            schema_version: SCHEMA_VERSION,
            ok: true,
            instance: "demo",
            duration_ms: 12,
            exit_status: 0,
            log_path: "/tmp/verify.log",
        })
        .unwrap();
        assert_eq!(json["ok"], true);
        assert_eq!(json["log_path"], "/tmp/verify.log");
    }

    #[test]
    fn down_ok_envelope_shape() {
        #[derive(Serialize)]
        struct DownOk {
            schema_version: u32,
            ok: bool,
            instance: &'static str,
            outcome: &'static str,
        }
        let json = serde_json::to_value(&DownOk {
            schema_version: SCHEMA_VERSION,
            ok: true,
            instance: "demo",
            outcome: "destroyed",
        })
        .unwrap();
        assert_eq!(json["ok"], true);
        assert_eq!(json["outcome"], "destroyed");
    }
}

impl ProgressSink for Output {
    fn on_step(&mut self, progress: StepProgress) {
        if self.json {
            #[derive(Serialize)]
            struct ProgressEvent<'a> {
                schema_version: u32,
                event: &'a str,
                instance: String,
                step: String,
                kind: stackless_core::engine::StepKind,
                node: String,
                index: usize,
                total: usize,
                #[serde(skip_serializing_if = "Option::is_none")]
                code: Option<&'static str>,
                at_epoch_ms: i64,
                #[serde(skip_serializing_if = "Option::is_none")]
                duration_ms: Option<u64>,
            }
            let event = match progress.event {
                StepProgressEvent::Started => "step_started",
                StepProgressEvent::Skipped => "step_skipped",
                StepProgressEvent::Completed => "step_completed",
                StepProgressEvent::Failed => "step_failed",
            };
            self.emit_ndjson(&ProgressEvent {
                schema_version: SCHEMA_VERSION,
                event,
                instance: progress.instance,
                step: progress.step_id,
                kind: progress.step_kind,
                node: progress.node,
                index: progress.index,
                total: progress.total,
                code: progress.code,
                at_epoch_ms: progress.at_epoch_ms,
                duration_ms: progress.duration_ms,
            });
            return;
        }
        let prefix = format!("{}: ", progress.instance);
        match progress.event {
            StepProgressEvent::Started => {
                eprintln!(
                    "{prefix}→ {} ({}/{})",
                    progress.step_id, progress.index, progress.total
                );
            }
            StepProgressEvent::Skipped => {
                eprintln!("{prefix}↷ {} (skipped)", progress.step_id);
            }
            StepProgressEvent::Completed => {
                eprintln!("{prefix}✓ {}", progress.step_id);
            }
            StepProgressEvent::Failed => {
                eprintln!("{prefix}✗ {}", progress.step_id);
            }
        }
    }
}
