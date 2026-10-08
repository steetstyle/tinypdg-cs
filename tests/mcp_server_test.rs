//! End-to-end MCP protocol test: launch the server binary, speak real JSON-RPC
//! over stdio, and call each tool.
//!
//! Unit tests cover the tool functions, but they never exercise the server
//! wiring: the handshake, capability negotiation, tool listing, argument
//! validation or error propagation. An agent's first contact with this server is
//! the protocol, so test that rather than the internals.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

/// A running `tiny-pdg-cs serve` process.
struct McpProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: i64,
}

impl McpProcess {
    fn start() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_tiny-pdg-cs"))
            .args(["serve"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn tiny-pdg-cs serve");

        let stdin = child.stdin.take().expect("stdin");
        let stdout = BufReader::new(child.stdout.take().expect("stdout"));
        Self {
            child,
            stdin,
            stdout,
            next_id: 0,
        }
    }

    fn send(&mut self, method: &str, params: serde_json::Value) -> serde_json::Value {
        self.next_id += 1;
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": self.next_id,
            "method": method,
            "params": params,
        });
        writeln!(self.stdin, "{request}").expect("write request");
        self.stdin.flush().expect("flush");

        // Responses arrive in order for this server; read until our id matches.
        loop {
            let mut line = String::new();
            let read = self.stdout.read_line(&mut line).expect("read response");
            assert!(read > 0, "server closed the stream before responding");
            let value: serde_json::Value = serde_json::from_str(line.trim()).unwrap_or_else(|e| {
                panic!("non-JSON line ({e}): {line}");
            });
            if value.get("id").and_then(|i| i.as_i64()) == Some(self.next_id) {
                return value;
            }
        }
    }

    fn call_tool(&mut self, name: &str, args: serde_json::Value) -> serde_json::Value {
        self.send(
            "tools/call",
            serde_json::json!({"name": name, "arguments": args}),
        )
    }

    /// Tool result payload, parsed from the single text content block.
    fn tool_json(result: &serde_json::Value) -> serde_json::Value {
        assert!(
            result.get("error").is_none(),
            "unexpected MCP error: {result}"
        );
        let content = result["result"]["content"]
            .as_array()
            .expect("content array");
        let text = content[0]["text"].as_str().expect("text content");
        serde_json::from_str(text).unwrap_or_else(|e| panic!("tool text is not JSON ({e}): {text}"))
    }
}

impl Drop for McpProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn write_fixture(name: &str, files: &[(&str, &str)]) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("tinylink_e2e_{}", name));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for (path, content) in files {
        let full = dir.join(path);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(full, content).unwrap();
    }
    dir
}

const FIXTURE: &[(&str, &str)] = &[
    (
        "Endpoints.cs",
        "namespace Shop.Api;\npublic class GetItemsEndpoint {\n  public static IResult Handler(HttpContext ctx) {\n    return Load();\n  }\n  static IResult Load() => null;\n}\n",
    ),
    (
        "Ext.cs",
        "var api = app.MapGroup(\"\");\napi.MapGet(\"/api/items/{id:guid}\", GetItemsEndpoint.Handler);\n",
    ),
    (
        "Service.cs",
        "namespace Shop.Api;\npublic class ItemService {\n  public void Fetch(int id) { }\n}\n",
    ),
];

#[test]
fn handshake_advertises_the_read_only_tools() {
    let mut mcp = McpProcess::start();

    let init = mcp.send(
        "initialize",
        serde_json::json!({
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {"name": "test", "version": "0"},
        }),
    );
    assert!(init.get("error").is_none(), "initialize failed: {init}");
    let caps = &init["result"]["capabilities"];
    assert!(
        caps.get("tools").is_some(),
        "server must advertise tools: {caps}"
    );

    mcp.send("notifications/initialized", serde_json::json!({}));

    let listed = mcp.send("tools/list", serde_json::json!({}));
    let tools = listed["result"]["tools"].as_array().expect("tools array");
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    for expected in [
        "project_summary",
        "list_routes",
        "find_callers",
        "method_pdg",
        "find_patterns",
    ] {
        assert!(names.contains(&expected), "missing {expected} in {names:?}");
    }
    assert_eq!(tools.len(), 7, "unexpected tool set: {names:?}");

    // An agent decides when to call a tool from its description, so an empty
    // one makes the tool unreachable in practice.
    for tool in tools {
        let desc = tool["description"].as_str().unwrap_or("");
        assert!(desc.len() > 40, "{} needs a real description", tool["name"]);
        assert_eq!(
            tool["annotations"]["readOnlyHint"],
            serde_json::json!(true),
            "{} must declare readOnlyHint",
            tool["name"]
        );
        // Argument schema must be advertised so the client knows what to send.
        assert!(
            !tool["inputSchema"].is_null(),
            "{} missing inputSchema",
            tool["name"]
        );
    }
}

#[test]
fn list_routes_tool_returns_minimal_api_route_over_the_protocol() {
    let dir = write_fixture("routes", FIXTURE);
    let mut mcp = McpProcess::start();
    mcp.send("initialize", serde_json::json!({
        "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "0"}
    }));
    mcp.send("notifications/initialized", serde_json::json!({}));

    let out = McpProcess::tool_json(&mcp.call_tool(
        "list_routes",
        serde_json::json!({"path": dir.display().to_string()}),
    ));
    assert_eq!(out["count"], 1, "{out}");
    assert_eq!(out["routes"][0]["class"], "GetItemsEndpoint");
    assert_eq!(out["routes"][0]["pattern"], "/api/items/{id}");
    assert_eq!(out["inline_lambda_routes"], 0);

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn project_summary_tool_counts_the_fixture() {
    let dir = write_fixture("summary", FIXTURE);
    let mut mcp = McpProcess::start();
    mcp.send("initialize", serde_json::json!({
        "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "0"}
    }));
    mcp.send("notifications/initialized", serde_json::json!({}));

    let out = McpProcess::tool_json(&mcp.call_tool(
        "project_summary",
        serde_json::json!({"path": dir.display().to_string()}),
    ));
    assert_eq!(out["classes"], 2, "{out}");
    assert!(out["methods"].as_u64().unwrap() >= 2, "{out}");
    assert_eq!(out["call_sites"], 1, "{out}");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn unknown_method_returns_an_error_and_the_server_survives() {
    let dir = write_fixture("errors", FIXTURE);
    let mut mcp = McpProcess::start();
    mcp.send("initialize", serde_json::json!({
        "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "0"}
    }));
    mcp.send("notifications/initialized", serde_json::json!({}));

    let bad = mcp.call_tool(
        "method_pdg",
        serde_json::json!({
            "path": dir.display().to_string(),
            "class": "GetItemsEndpoint",
            "method": "NoSuchMethod",
        }),
    );
    assert!(
        bad.get("error").is_some(),
        "unknown method must produce an MCP error: {bad}"
    );

    // A failed call must not poison the session: the next call has to work.
    let good = McpProcess::tool_json(&mcp.call_tool(
        "project_summary",
        serde_json::json!({"path": dir.display().to_string()}),
    ));
    assert_eq!(good["classes"], 2, "server must stay usable after an error");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn missing_required_argument_is_rejected() {
    let mut mcp = McpProcess::start();
    mcp.send("initialize", serde_json::json!({
        "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "0"}
    }));
    mcp.send("notifications/initialized", serde_json::json!({}));

    // `path` is required; omitting it must be reported, not panic.
    // MCP distinguishes two failure shapes: a JSON-RPC `error` object, and a
    // tool result flagged `isError`. Both are acceptable here — what matters is
    // that the client is told instead of getting a silent success.
    let bad = mcp.call_tool("list_routes", serde_json::json!({}));
    let reported =
        bad.get("error").is_some() || bad["result"]["isError"] == serde_json::json!(true);
    assert!(
        reported,
        "missing required argument must be reported to the client: {bad}"
    );
    let detail = bad.get("error").map(|e| e.to_string()).unwrap_or_else(|| {
        bad["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or("")
            .to_string()
    });
    assert!(
        detail.contains("path"),
        "the error should name the missing field: {detail}"
    );
}

#[test]
fn method_pdg_tool_returns_blocks_over_the_protocol() {
    let dir = write_fixture("pdg", FIXTURE);
    let mut mcp = McpProcess::start();
    mcp.send("initialize", serde_json::json!({
        "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "0"}
    }));
    mcp.send("notifications/initialized", serde_json::json!({}));

    let out = McpProcess::tool_json(&mcp.call_tool(
        "method_pdg",
        serde_json::json!({
            "path": dir.display().to_string(),
            "class": "GetItemsEndpoint",
            "method": "Handler",
        }),
    ));
    assert_eq!(out["method"], "Handler");
    let blocks = out["blocks"].as_array().unwrap();
    assert!(!blocks.is_empty(), "expected PDG blocks: {out}");
    // Structured output, not DOT: an agent must not have to parse a graphviz
    // string.
    assert!(blocks[0]["kind"].is_string(), "{out}");
    assert!(out["edges"].is_array(), "{out}");

    std::fs::remove_dir_all(&dir).ok();
}
