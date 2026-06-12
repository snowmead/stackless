//! Human and `--json` output. JSON goes to stdout, human prose to
//! stderr for errors — agents parse stdout, people read stderr.

use serde::Serialize;

use stackless_core::def::{DependencyGraph, StackDef};
use stackless_core::engine::{ProgressSink, StepProgress, StepProgressEvent};
use stackless_core::fault::{ErrorContext, Fault, Report};

pub struct Output {
    json: bool,
}

#[derive(Serialize)]
struct CheckOk<'a> {
    ok: bool,
    stack: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    substrate: Option<&'a str>,
    services: Vec<&'a str>,
    datastores: Vec<&'a str>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    persistence_warning: Option<&'a str>,
    #[serde(flatten)]
    report: &'a crate::commands::InstanceStatusReport,
}

/// `list --json`: the same warning alongside the instance array.
#[derive(Serialize)]
struct ListEnvelope<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    persistence_warning: Option<&'a str>,
    instances: &'a [crate::commands::InstanceStatusReport],
}

impl Output {
    pub fn new(json: bool) -> Self {
        Self { json }
    }

    pub fn is_json(&self) -> bool {
        self.json
    }

    pub fn check_ok(&self, def: &StackDef, graph: &DependencyGraph, substrate: Option<&str>) {
        if self.json {
            self.emit(&CheckOk {
                ok: true,
                stack: def.stack.name.as_str(),
                substrate,
                services: def.services.keys().map(String::as_str).collect(),
                datastores: def.datastores.keys().map(String::as_str).collect(),
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
        if !def.datastores.is_empty() {
            println!(
                "  datastores: {}",
                def.datastores
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
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

    pub fn up_ok(
        &self,
        name: &str,
        substrate: &str,
        outcome: &stackless_core::engine::UpOutcome,
        origins: &[(String, String)],
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
                origins: Vec<Origin<'a>>,
            }
            #[derive(Serialize)]
            struct Origin<'a> {
                service: &'a str,
                origin: &'a str,
            }
            self.emit(&UpOk {
                schema_version: 1,
                ok: true,
                instance: name,
                substrate,
                executed: &outcome.executed,
                skipped: &outcome.skipped,
                origins: origins
                    .iter()
                    .map(|(service, origin)| Origin { service, origin })
                    .collect(),
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
            eprintln!("{text}");
        } else {
            println!("{text}");
        }
    }

    pub fn logs_json(&self, instance: &str, services: &[LogService<'_>]) {
        #[derive(Serialize)]
        struct LogsOk<'a> {
            ok: bool,
            instance: &'a str,
            services: &'a [LogService<'a>],
        }
        self.emit(&LogsOk {
            ok: true,
            instance,
            services,
        });
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
            Ok(json) => println!("{json}"),
            // Serialization of our own types cannot fail; if it ever
            // does, say so on stderr rather than emitting half-JSON.
            Err(err) => eprintln!("error[cli.json.serialize]: {err}"),
        }
    }

    fn emit_ndjson<T: Serialize>(&self, value: &T) {
        match serde_json::to_string(value) {
            Ok(json) => eprintln!("{json}"),
            Err(err) => eprintln!("error[cli.json.serialize]: {err}"),
        }
    }
}

#[derive(Serialize)]
pub struct LogService<'a> {
    pub service: &'a str,
    pub source: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_path: Option<String>,
    pub lines: Vec<String>,
}

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
        };
        #[derive(Serialize)]
        struct LogsOk<'a> {
            ok: bool,
            instance: &'a str,
            services: &'a [LogService<'a>],
        }
        let json = serde_json::to_value(&LogsOk {
            ok: true,
            instance: "demo",
            services: &[entry],
        })
        .unwrap();
        assert_eq!(json["ok"], true);
        assert_eq!(json["instance"], "demo");
        assert_eq!(json["services"][0]["source"], "file");
        assert_eq!(json["services"][0]["log_path"], "/tmp/demo/web.log");
        assert_eq!(json["services"][0]["lines"][0], "listening on :3000");
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
            }
            let event = match progress.event {
                StepProgressEvent::Started => "step_started",
                StepProgressEvent::Skipped => "step_skipped",
                StepProgressEvent::Completed => "step_completed",
                StepProgressEvent::Failed => "step_failed",
            };
            self.emit_ndjson(&ProgressEvent {
                schema_version: 1,
                event,
                instance: progress.instance,
                step: progress.step_id,
                kind: progress.step_kind,
                node: progress.node,
                index: progress.index,
                total: progress.total,
                code: progress.code,
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
