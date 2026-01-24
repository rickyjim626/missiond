//! mission-mcp - MCP stdio server proxy for missiond daemon
//!
//! This binary is intended to be launched by Claude Code as an MCP server.
//! It forwards tool calls to a singleton `missiond` daemon over IPC.
//! - Unix: Uses Unix domain sockets
//! - Windows: Uses TCP loopback

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::time::sleep;
use tracing::{debug, error, info, warn};

use missiond_core::ipc::{self, IpcStream};
use missiond_mcp::protocol::{Request, RequestId, Response, JSONRPC_VERSION};
use missiond_mcp::server::{McpServer, ToolHandler};
use missiond_mcp::tools::ToolResult;

static NEXT_ID: AtomicI64 = AtomicI64::new(1);

#[derive(Clone)]
struct IpcClient {
    endpoint: String,
}

impl IpcClient {
    async fn call_tool(&self, name: &str, arguments: Value) -> Result<ToolResult> {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);

        let request = Request {
            jsonrpc: JSONRPC_VERSION.to_string(),
            method: "tools/call".to_string(),
            params: Some(serde_json::json!({
                "name": name,
                "arguments": arguments,
            })),
            id: RequestId::Number(id),
        };

        let mut stream = IpcStream::connect(&self.endpoint)
            .await
            .with_context(|| format!("Failed to connect to daemon: {}", self.endpoint))?;

        let request_json = serde_json::to_string(&request)?;
        debug!(%name, "IPC -> {}", request_json);
        stream.write_all(request_json.as_bytes()).await?;
        stream.write_all(b"\n").await?;
        stream.flush().await?;

        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        let bytes = reader.read_line(&mut line).await?;
        if bytes == 0 {
            return Err(anyhow!("Daemon closed connection without response"));
        }
        let line = line.trim();
        debug!(%name, "IPC <- {}", line);

        let response: Response = serde_json::from_str(line)?;
        if let Some(err) = response.error {
            return Ok(ToolResult::error(err.message));
        }

        let result = response
            .result
            .ok_or_else(|| anyhow!("Missing result in daemon response"))?;
        let tool_result: ToolResult = serde_json::from_value(result)?;
        Ok(tool_result)
    }
}

struct ProxyHandler {
    client: IpcClient,
}

#[async_trait::async_trait]
impl ToolHandler for ProxyHandler {
    async fn call(&self, name: &str, arguments: Value) -> ToolResult {
        match self.client.call_tool(name, arguments).await {
            Ok(res) => res,
            Err(e) => {
                error!(tool = %name, error = %e, "IPC tool call failed");
                ToolResult::error(e.to_string())
            }
        }
    }
}

fn log_filter() -> tracing_subscriber::EnvFilter {
    let level = if let Ok(v) = std::env::var("RUST_LOG") {
        v
    } else if let Ok(v) = std::env::var("MISSION_LOG_LEVEL") {
        match v.as_str() {
            "silent" => "off".to_string(),
            "fatal" => "error".to_string(),
            other => other.to_string(),
        }
    } else {
        "warn".to_string()
    };

    tracing_subscriber::EnvFilter::try_new(level)
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"))
}

fn ipc_endpoint_from_env() -> String {
    if let Ok(endpoint) = std::env::var("MISSION_IPC_ENDPOINT") {
        return endpoint;
    }
    // Legacy support for MISSION_IPC_SOCKET on Unix
    #[cfg(unix)]
    if let Ok(socket) = std::env::var("MISSION_IPC_SOCKET") {
        return socket;
    }
    ipc::default_ipc_endpoint()
}

fn missiond_binary_path() -> PathBuf {
    #[cfg(windows)]
    const BINARY_NAME: &str = "missiond.exe";
    #[cfg(not(windows))]
    const BINARY_NAME: &str = "missiond";

    // Prefer a sibling binary next to the current executable (dev / cargo build).
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join(BINARY_NAME);
            if candidate.exists() {
                return candidate;
            }
        }
    }
    PathBuf::from(BINARY_NAME)
}

fn spawn_daemon() -> Result<()> {
    let bin = missiond_binary_path();
    let mut cmd = std::process::Command::new(&bin);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .envs(std::env::vars());

    cmd.spawn()
        .with_context(|| format!("Failed to spawn daemon: {}", bin.display()))?;
    Ok(())
}

async fn ensure_daemon(endpoint: &str) -> Result<()> {
    if IpcStream::can_connect(endpoint).await {
        return Ok(());
    }

    warn!(
        endpoint = %endpoint,
        "Daemon not reachable, starting daemon"
    );
    spawn_daemon()?;

    // Wait for daemon to come up
    for _ in 0..50 {
        if IpcStream::can_connect(endpoint).await {
            info!("Daemon is ready");
            return Ok(());
        }
        sleep(Duration::from_millis(100)).await;
    }

    Err(anyhow!(
        "Timed out waiting for daemon: {}",
        endpoint
    ))
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(log_filter())
        .with_writer(std::io::stderr)
        .init();

    let endpoint = ipc_endpoint_from_env();
    ensure_daemon(&endpoint).await?;

    let handler = ProxyHandler {
        client: IpcClient { endpoint },
    };
    let mut server = McpServer::new(handler);
    server.run().await?;
    Ok(())
}
