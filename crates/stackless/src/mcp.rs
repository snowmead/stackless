//! Hidden `stackless mcp` — stdio JSON-RPC MCP server exposing lifecycle
//! tools as thin wrappers over existing command functions (`--json` forced).

use std::io::{self, BufRead, Write};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::client::{Client, UpArgs};
use crate::doctor;
use crate::error::Error;
use crate::output::{self, Output};
use crate::verify;

const PROTOCOL_VERSION: &str = "2024-11-05";

pub fn run_stdio_server() -> io::Result<()> {
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    for line in stdin.lock().lines() {
        let line = line?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let request: JsonRpcRequest = match serde_json::from_str(trimmed) {
            Ok(req) => req,
            Err(err) => {
                let response = JsonRpcResponse::error(None, -32700, format!("parse error: {err}"));
                write_response(&mut stdout, &response)?;
                continue;
            }
        };
        if let Some(response) = handle_request(&request) {
            write_response(&mut stdout, &response)?;
        }
    }
    Ok(())
}

fn write_response(stdout: &mut impl Write, response: &JsonRpcResponse) -> io::Result<()> {
    let json = serde_json::to_string(response).map_err(io::Error::other)?;
    writeln!(stdout, "{json}")?;
    stdout.flush()
}

#[derive(Debug, Deserialize)]
struct JsonRpcRequest {
    #[serde(default)]
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Serialize)]
struct JsonRpcResponse {
    jsonrpc: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

#[derive(Serialize)]
struct JsonRpcError {
    code: i32,
    message: String,
}

impl JsonRpcResponse {
    fn ok(id: Option<Value>, result: Value) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        }
    }

    fn error(id: Option<Value>, code: i32, message: String) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(JsonRpcError { code, message }),
        }
    }
}

fn handle_request(request: &JsonRpcRequest) -> Option<JsonRpcResponse> {
    request.id.as_ref()?;
    let id = request.id.clone();
    match request.method.as_str() {
        "initialize" => Some(handle_initialize(id)),
        "tools/list" => Some(handle_tools_list(id)),
        "tools/call" => Some(handle_tools_call(id, &request.params)),
        "ping" => Some(JsonRpcResponse::ok(id, json!({}))),
        other => Some(JsonRpcResponse::error(
            id,
            -32601,
            format!("method not found: {other}"),
        )),
    }
}

fn handle_initialize(id: Option<Value>) -> JsonRpcResponse {
    JsonRpcResponse::ok(
        id,
        json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {
                "tools": {}
            },
            "serverInfo": {
                "name": "stackless",
                "version": env!("CARGO_PKG_VERSION"),
            }
        }),
    )
}

fn handle_tools_list(id: Option<Value>) -> JsonRpcResponse {
    JsonRpcResponse::ok(id, json!({ "tools": tool_definitions() }))
}

fn handle_tools_call(id: Option<Value>, params: &Value) -> JsonRpcResponse {
    let name = match params.get("name").and_then(Value::as_str) {
        Some(name) => name,
        None => {
            return JsonRpcResponse::error(id, -32602, "tools/call requires params.name".into());
        }
    };
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    match dispatch_tool(name, &arguments) {
        Ok(result) => JsonRpcResponse::ok(id, result),
        Err(message) => JsonRpcResponse::error(id, -32000, message),
    }
}

#[derive(Serialize)]
struct ToolDefinition {
    name: &'static str,
    description: &'static str,
    #[serde(rename = "inputSchema")]
    input_schema: Value,
}

fn tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "stackless_check",
            description: "Parse and validate a stackless.toml; print the derived graph.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "file": { "type": "string", "description": "Path to stackless.toml" },
                    "substrate": { "type": "string", "description": "Also validate for substrate (local, render, vercel, fly, netlify)" }
                },
                "required": ["file"]
            }),
        },
        ToolDefinition {
            name: "stackless_doctor",
            description: "Preflight checks: daemon, env keys, Stripe Projects.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "file": { "type": "string", "description": "Definition file (default ./stackless.toml)" },
                    "substrate": { "type": "string", "description": "Also check substrate-specific API keys" }
                }
            }),
        },
        ToolDefinition {
            name: "stackless_up",
            description: "Create or resume a named stack instance; health-gated.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Instance name (DNS-safe)" },
                    "file": { "type": "string", "description": "Definition file path" },
                    "on": { "type": "string", "description": "Substrate at creation (local, render, vercel, fly, netlify)" },
                    "sources": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Source pins: SERVICE or SERVICE=PATH"
                    },
                    "dirty": { "type": "boolean", "description": "Snapshot --source pins (local-only)" },
                    "lease": { "type": "string", "description": "Lease duration, e.g. 8h" },
                    "confirm_paid": { "type": "boolean", "description": "Consent to paid cloud resources" }
                }
            }),
        },
        ToolDefinition {
            name: "stackless_down",
            description: "Verified teardown of a named instance.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Instance name" }
                },
                "required": ["name"]
            }),
        },
        ToolDefinition {
            name: "stackless_verify",
            description: "Run the stack proof contract; renews the lease.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Instance name" },
                    "tier": { "type": "string", "description": "Named verify tier" }
                },
                "required": ["name"]
            }),
        },
        ToolDefinition {
            name: "stackless_status",
            description: "Staged truth per service for a live instance.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Instance name" }
                },
                "required": ["name"]
            }),
        },
        ToolDefinition {
            name: "stackless_list",
            description: "List all instances with lease remaining.",
            input_schema: json!({
                "type": "object",
                "properties": {}
            }),
        },
        ToolDefinition {
            name: "stackless_logs",
            description: "Tail captured service output for an instance.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Instance name" },
                    "service": { "type": "string", "description": "Single service (default: all)" },
                    "tail": { "type": "integer", "description": "Lines per service (default 100)" }
                },
                "required": ["name"]
            }),
        },
    ]
}

fn dispatch_tool(name: &str, args: &Value) -> Result<Value, String> {
    match name {
        "stackless_check" => {
            let file = require_str(args, "file")?;
            let substrate = optional_str(args, "substrate");
            run_command(|output| {
                let client = Client::system()?;
                let outcome = client.check(&PathBuf::from(file), substrate.as_deref())?;
                output::render_check(output, &PathBuf::from(file), &outcome)
            })
        }
        "stackless_doctor" => {
            let file = optional_path(args, "file");
            let substrate = optional_str(args, "substrate");
            run_command(|output| doctor::doctor(doctor::DoctorArgs { file, substrate }, output))
        }
        "stackless_up" => {
            let name = optional_str(args, "name");
            let file = optional_path(args, "file");
            let on = optional_str(args, "on");
            let sources = optional_string_array(args, "sources");
            let dirty = args.get("dirty").and_then(Value::as_bool).unwrap_or(false);
            let lease = optional_str(args, "lease");
            let confirm_paid = args
                .get("confirm_paid")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            run_command_mut(|output| {
                let client = Client::system()?;
                let outcome = client.up_from_args_with_progress(
                    UpArgs {
                        name,
                        file,
                        on,
                        sources,
                        dirty,
                        lease,
                        confirm_paid,
                    },
                    Some(output),
                )?;
                output::render_up(output, &outcome);
                Ok(())
            })
        }
        "stackless_down" => {
            let name = require_str(args, "name")?;
            run_command(|output| {
                let client = Client::system()?;
                let outcome = client.down(name)?;
                output::render_down(output, &outcome);
                Ok(())
            })
        }
        "stackless_verify" => {
            let name = require_str(args, "name")?.to_owned();
            let tier = optional_str(args, "tier");
            run_command(|output| verify::verify(verify::VerifyArgs { name, tier }, output))
        }
        "stackless_status" => {
            let name = require_str(args, "name")?;
            run_command(|output| {
                let client = Client::system()?;
                let report = client.status(name)?;
                output::render_status(output, &report, client.paths());
                Ok(())
            })
        }
        "stackless_list" => run_command(|output| {
            let client = Client::system()?;
            let reports = client.list()?;
            output::render_list(output, &reports, client.paths());
            Ok(())
        }),
        "stackless_logs" => {
            let name = require_str(args, "name")?;
            let service = optional_str(args, "service");
            let tail = args
                .get("tail")
                .and_then(Value::as_u64)
                .map(|n| n as usize)
                .unwrap_or(100);
            run_command(|output| {
                let client = Client::system()?;
                let outcome = client.logs(name, service.as_deref(), tail)?;
                output::render_logs(output, &outcome);
                Ok(())
            })
        }
        other => Err(format!("unknown tool: {other}")),
    }
}

fn run_command(f: impl FnOnce(&Output) -> Result<(), Error>) -> Result<Value, String> {
    let output = Output::capturing_json();
    let result = f(&output);
    tool_result(output, result)
}

fn run_command_mut(f: impl FnOnce(&mut Output) -> Result<(), Error>) -> Result<Value, String> {
    let mut output = Output::capturing_json();
    let result = f(&mut output);
    tool_result(output, result)
}

fn tool_result(output: Output, result: Result<(), Error>) -> Result<Value, String> {
    let (stdout, stderr) = output.take_capture();
    let is_error = result.is_err();
    let text = if let Err(err) = &result {
        if stdout.trim().is_empty() {
            let fallback = Output::capturing_json();
            fallback.fault(err);
            let (fb_stdout, _) = fallback.take_capture();
            combine_capture(&stderr, &fb_stdout)
        } else {
            combine_capture(&stderr, &stdout)
        }
    } else {
        combine_capture(&stderr, &stdout)
    };
    Ok(tool_content(&text, is_error))
}

fn combine_capture(stderr: &str, stdout: &str) -> String {
    match (stderr.trim().is_empty(), stdout.trim().is_empty()) {
        (true, true) => String::new(),
        (false, true) => stderr.to_owned(),
        (true, false) => stdout.to_owned(),
        (false, false) => format!("{stderr}\n---\n{stdout}"),
    }
}

fn tool_content(text: &str, is_error: bool) -> Value {
    json!({
        "content": [{ "type": "text", "text": text }],
        "isError": is_error
    })
}

fn require_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("missing required argument: {key}"))
}

fn optional_str(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn optional_path(args: &Value, key: &str) -> Option<PathBuf> {
    optional_str(args, key).map(PathBuf::from)
}

fn optional_string_array(args: &Value, key: &str) -> Vec<String> {
    args.get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tools_list_returns_all_lifecycle_tools() {
        let response = handle_tools_list(Some(json!(1)));
        let result = response.result.expect("result");
        let tools = result["tools"].as_array().expect("tools array");
        let names: Vec<_> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
        assert!(names.contains(&"stackless_check"));
        assert!(names.contains(&"stackless_up"));
        assert!(names.contains(&"stackless_list"));
        assert_eq!(tools.len(), 8);
    }

    #[test]
    fn initialize_reports_stackless_server_info() {
        let response = handle_initialize(Some(json!(1)));
        let result = response.result.expect("result");
        assert_eq!(result["serverInfo"]["name"], "stackless");
        assert_eq!(result["protocolVersion"], PROTOCOL_VERSION);
    }

    #[test]
    fn unknown_tool_returns_error_result() {
        let err = dispatch_tool("nope", &json!({})).unwrap_err();
        assert!(err.contains("unknown tool"));
    }
}
