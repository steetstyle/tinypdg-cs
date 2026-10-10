//! End-to-end Streamable HTTP test: bind a real socket, talk real MCP over it.
//!
//! The stdio tests cover the protocol; this covers the transport. The two fail
//! differently — a bind problem, a wrong mount path, a missing session header, a
//! client that hangs on the SSE stream — and none of those show up without
//! actually listening on a port.

use std::net::SocketAddr;

use rmcp::transport::StreamableHttpClientTransport;
use rmcp::ServiceExt;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

/// Start the server on an ephemeral port and return its address.
async fn serve_http() -> SocketAddr {
    use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
    use rmcp::transport::streamable_http_server::tower::StreamableHttpService;
    use tiny_pdg_cs::mcp::AnalysisServer;

    let service: StreamableHttpService<AnalysisServer, LocalSessionManager> =
        StreamableHttpService::new(
            || Ok(AnalysisServer::new()),
            Default::default(),
            Default::default(),
        );
    let router = axum::Router::new().nest_service("/mcp", service);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("local addr");

    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });

    addr
}

fn fixture_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("tinylink_http_{}", name));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("E.cs"),
        "app.MapGet(\"/api/thing/{id}\", ThingEndpoint.Handler);\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("T.cs"),
        "namespace N;\npublic class ThingEndpoint { public static IResult Handler() => null; }\n",
    )
    .unwrap();
    dir
}

/// Minimal MCP client over HTTP POST, using the protocol by hand.
///
/// Deliberately not using the SDK client: this asserts the server responds
/// correctly to a plain POST with the documented headers, which is what a
/// third-party client sends.
async fn post(
    addr: SocketAddr,
    path: &str,
    body: &Value,
    session: Option<&str>,
) -> (u16, Value, Option<String>) {
    let mut stream = TcpStream::connect(addr).await.expect("connect");
    let payload = serde_json::to_string(body).expect("serialize");

    let mut request = format!(
        "POST {path} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\n\
         Accept: application/json, text/event-stream\r\nContent-Length: {}\r\n",
        payload.len()
    );
    if let Some(s) = session {
        request.push_str(&format!("Mcp-Session-Id: {s}\r\n"));
    }
    request.push_str("\r\n");
    request.push_str(&payload);

    stream.write_all(request.as_bytes()).await.expect("write");
    stream.flush().await.expect("flush");

    let mut reader = BufReader::new(stream);
    let mut status_line = String::new();
    reader
        .read_line(&mut status_line)
        .await
        .expect("status line");

    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);

    let mut session_id = None;
    let mut content_length: Option<usize> = None;
    let mut chunked = false;
    loop {
        let mut line = String::new();
        let read = reader.read_line(&mut line).await.expect("header");
        if read == 0 || line == "\r\n" || line == "\n" {
            break;
        }
        let lower = line.to_lowercase();
        if let Some(v) = lower.strip_prefix("mcp-session-id:") {
            session_id = Some(v.trim().to_string());
        }
        if let Some(v) = lower.strip_prefix("content-length:") {
            content_length = v.trim().parse().ok();
        }
        if lower.starts_with("transfer-encoding:") && lower.contains("chunked") {
            chunked = true;
        }
    }

    // Read the body: either Content-Length delimited or chunked. The server
    // may answer with a single SSE event, so scan for the JSON payload.
    let mut raw = String::new();
    if chunked {
        loop {
            let mut size_line = String::new();
            if reader.read_line(&mut size_line).await.unwrap_or(0) == 0 {
                break;
            }
            let size = usize::from_str_radix(size_line.trim(), 16).unwrap_or(0);
            if size == 0 {
                break;
            }
            let mut chunk = vec![0u8; size];
            tokio::io::AsyncReadExt::read_exact(&mut reader, &mut chunk)
                .await
                .expect("chunk");
            raw.push_str(&String::from_utf8_lossy(&chunk));
            let mut crlf = String::new();
            let _ = reader.read_line(&mut crlf).await;
        }
    } else if let Some(len) = content_length {
        let mut buf = vec![0u8; len];
        tokio::io::AsyncReadExt::read_exact(&mut reader, &mut buf)
            .await
            .expect("body");
        raw = String::from_utf8_lossy(&buf).to_string();
    } else {
        // Connection-close framing.
        let mut buf = Vec::new();
        let _ = tokio::io::AsyncReadExt::read_to_end(&mut reader, &mut buf).await;
        raw = String::from_utf8_lossy(&buf).to_string();
    }

    // An SSE body wraps the JSON in `data:` lines; a JSON body is the payload.
    let json = raw
        .lines()
        .filter_map(|l| l.strip_prefix("data:").map(str::trim))
        .find(|l| l.starts_with('{'))
        .map(|l| l.to_string())
        .or_else(|| raw.find('{').map(|i| raw[i..].trim().to_string()))
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(Value::Null);

    (status, json, session_id)
}

fn init_request(id: i64) -> Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {"name": "http-test", "version": "0"}
        }
    })
}

#[tokio::test]
async fn initialize_over_http_returns_tools_capability_and_a_session() {
    let addr = serve_http().await;

    let (status, body, session) = post(addr, "/mcp", &init_request(1), None).await;

    assert_eq!(status, 200, "initialize must succeed over HTTP: {body}");
    assert!(body.get("error").is_none(), "{body}");
    assert!(
        body["result"]["capabilities"]["tools"].is_object(),
        "tools capability must be advertised: {body}"
    );
    // The spec requires a session id for streamable HTTP.
    assert!(
        session.is_some(),
        "streamable HTTP must return Mcp-Session-Id, got none"
    );

    let server_info = &body["result"]["serverInfo"];
    assert!(server_info.is_object(), "{body}");
}

#[tokio::test]
async fn tools_are_callable_over_http() {
    let addr = serve_http().await;
    let dir = fixture_dir("call");

    let (_, init, session) = post(addr, "/mcp", &init_request(1), None).await;
    assert!(init.get("error").is_none(), "{init}");
    let session = session.expect("session id");

    // Both signals are required by the streamable HTTP transport.
    let (status, body, _) = post(
        addr,
        "/mcp",
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        }),
        Some(&session),
    )
    .await;

    assert_eq!(status, 200, "tools/list failed: {body}");
    let tools = body["result"]["tools"].as_array().expect("tools array");
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    // LISTED_TOOLS, not a number written here. This test asserts that the HTTP transport
    // does not lose tools, and it was asserting 8 while the router served 9 -- because
    // search_github was added and this number was not. A hand-kept count cannot tell a
    // lost tool from an added one, which is the only thing this test is for.
    for expected in tiny_pdg_cs::mcp::LISTED_TOOLS {
        assert!(names.contains(&expected), "missing {expected} in {names:?}");
    }
    assert_eq!(
        names.len(),
        tiny_pdg_cs::mcp::LISTED_TOOLS.len(),
        "{names:?}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn a_tool_call_over_http_returns_structured_json() {
    let addr = serve_http().await;
    let dir = fixture_dir("tool_call");

    let (_, _, session) = post(addr, "/mcp", &init_request(1), None).await;
    let session = session.expect("session id");

    let (status, body, _) = post(
        addr,
        "/mcp",
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "list_routes",
                "arguments": {"path": dir.display().to_string()}
            }
        }),
        Some(&session),
    )
    .await;

    assert_eq!(status, 200, "{body}");
    assert!(body.get("error").is_none(), "{body}");
    let text = body["result"]["content"][0]["text"]
        .as_str()
        .expect("text content");
    let payload: Value = serde_json::from_str(text).unwrap_or_else(|e| panic!("{e}: {text}"));
    assert_eq!(payload["count"], 1, "{payload}");
    assert_eq!(payload["routes"][0]["class"], "ThingEndpoint");

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn a_request_without_a_session_id_is_rejected() {
    let addr = serve_http().await;

    // Skipping the initialized notification and session id must not be accepted:
    // without it the server cannot correlate the request to a session.
    let (status, body, _) = post(
        addr,
        "/mcp",
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": 9,
            "method": "tools/list",
            "params": {}
        }),
        None,
    )
    .await;

    assert!(
        status >= 400,
        "a sessionless request must be rejected, got {status}: {body}"
    );
}

#[tokio::test]
async fn wrong_mount_path_does_not_answer() {
    let addr = serve_http().await;

    let (status, _, _) = post(addr, "/wrong", &init_request(1), None).await;
    assert!(
        status == 404 || status == 405 || status >= 400,
        "an unmounted path must not serve MCP, got {status}"
    );
}

#[tokio::test]
async fn sdk_client_can_drive_the_http_server() {
    // The hand-rolled client above proves the wire format; this proves the
    // official SDK client interoperates with it, which is what an agent uses.
    let addr = serve_http().await;
    let url: std::sync::Arc<str> = format!("http://{addr}/mcp").into();

    let client =
        ().serve(StreamableHttpClientTransport::from_uri(url))
            .await
            .expect("SDK client must connect over streamable HTTP");

    let tools = client.list_all_tools().await.expect("list tools");
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
    assert_eq!(
        names.len(),
        tiny_pdg_cs::mcp::LISTED_TOOLS.len(),
        "{names:?}"
    );
    assert!(names.contains(&"list_routes"), "{names:?}");

    client.cancel().await.ok();
}
