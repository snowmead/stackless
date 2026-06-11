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
