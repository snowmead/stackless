//! Human and `--json` output. JSON goes to stdout, human prose to
//! stderr for errors — agents parse stdout, people read stderr.

use serde::Serialize;

use stackless_core::def::{DependencyGraph, StackDef};
use stackless_core::fault::{Fault, Report};

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

    pub fn check_ok(&self, def: &StackDef, graph: &DependencyGraph, substrate: Option<&str>) {
        if self.json {
            self.emit(&CheckOk {
                ok: true,
                stack: &def.stack.name,
                substrate,
                services: def.services.keys().map(String::as_str).collect(),
                datastores: def.datastores.keys().map(String::as_str).collect(),
                graph,
            });
            return;
        }
        println!("stack {:?}: valid", def.stack.name);
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

    pub fn fault(&self, fault: &dyn Fault) {
        if self.json {
            self.emit(&ErrorEnvelope {
                ok: false,
                error: Report::from_fault(fault),
            });
            return;
        }
        eprintln!("error[{}]: {fault}", fault.code());
        eprintln!("  remediation: {}", fault.remediation());
    }

    fn emit<T: Serialize>(&self, value: &T) {
        match serde_json::to_string_pretty(value) {
            Ok(json) => println!("{json}"),
            // Serialization of our own types cannot fail; if it ever
            // does, say so on stderr rather than emitting half-JSON.
            Err(err) => eprintln!("error[cli.json.serialize]: {err}"),
        }
    }
}
