mod db;
mod protocol;
mod tools;
mod config;

use anyhow::Result;
use db::{ConfigSharedState, ConfigVsDBstate};
use protocol::{JsonRpcRequest, JsonRpcResponse};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    sync::Mutex,
};
use tracing::{debug, error, info, warn};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            std::env::var("RUST_LOG")
                .unwrap_or_else(|_| "db_mcp=info".into()),
        )
        .init();
    sqlx::any::install_default_drivers();

    info!("db-mcp server starting");
    if let Err(e) = config::initialize_config() {
        eprintln!("db-mcp: failed to initialize config: {e}");
    }
    let state_holder: ConfigSharedState = Arc::new(Mutex::new(ConfigVsDBstate::new()));
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let mut reader = BufReader::new(stdin);
    let mut writer = stdout;
    let mut line = String::new();

    loop {
        line.clear();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            info!("stdin closed, shutting down");
            break;
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        debug!("← {trimmed}");

        let request: JsonRpcRequest = match serde_json::from_str(trimmed) {
            Ok(r) => r,
            Err(e) => {
                error!("Failed to parse JSON-RPC request: {e}");
                let resp = JsonRpcResponse::err(None, -32700, format!("Parse error: {e}"));
                send(&mut writer, &resp).await?;
                continue;
            }
        };

        if request.id.is_none() {
            info!("Notification: {}", request.method);
            continue;
        }

        let id = request.id.clone();
        let response = handle(&request, &state_holder).await;
        let resp = match response {
            Ok(result) => JsonRpcResponse::ok(id, result),
            Err(e) => JsonRpcResponse::err(id, -32603, e.to_string()),
        };

        send(&mut writer, &resp).await?;
    }

    Ok(())
}

async fn handle(
    req: &JsonRpcRequest,
    state: &ConfigSharedState,
) -> Result<Value> {
    match req.method.as_str() {
        "initialize" => {
            info!("Client initialised");
            Ok(json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {
                    "tools": {}
                },
                "serverInfo": {
                    "name": "db-mcp",
                    "version": env!("CARGO_PKG_VERSION"),
                    "description": "Connect MySQL / PostgreSQL Server with LLM Agents"
                }
            }))
        }

        "ping" => Ok(json!({})),

        "tools/list" => Ok(tools::tool_list()),

        "tools/call" => {
            let params = req.params.as_ref().ok_or_else(|| {
                anyhow::anyhow!("tools/call requires params")
            })?;

            let name = params
                .get("name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("tools/call: missing name"))?;

            let args = params.get("arguments").cloned().unwrap_or(json!({}));

            info!("Tool call: {name}");
            let result = tools::dispatch(name, &args, state).await;
            Ok(result)
        }

        other => {
            warn!("Unknown method: {other}");
            Err(anyhow::anyhow!("Method not found: {other}"))
        }
    }
}

async fn send<W: AsyncWriteExt + Unpin>(writer: &mut W, resp: &JsonRpcResponse,) -> Result<()> {
    let mut json = serde_json::to_string(resp)?;
    json.push('\n');
    debug!("→ {}", json.trim());
    writer.write_all(json.as_bytes()).await?;
    writer.flush().await?;
    Ok(())
}
