//! missiond - singleton daemon for missiond
//!
//! Responsibilities:
//! - Own the global state (DB, slot/process/task/inbox, PTY sessions, CC tasks watcher)
//! - Provide a stable WebSocket endpoint for attach + tasks events
//! - Expose an IPC JSON-RPC endpoint for MCP proxy processes

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, Result};
use missiond_core::{
    CorePermissionDecision, MissionControl, MissionControlOptions, PermissionPolicy, PermissionRule,
    PTYManager, PTYSpawnOptions, PTYWebSocketServer, WSServerOptions,
};
use missiond_core::{CCTasksWatcher, CCTasksWatcherOptions};
use missiond_mcp::protocol::{self, Request, RequestId, Response, RpcError};
use missiond_mcp::tools::ToolResult;
use serde::Deserialize;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;
use tracing::{error, info, warn};

use missiond_core::ipc::{self, IpcListener, IpcStream};

#[derive(Clone)]
struct AppState {
    mission: Arc<MissionControl>,
    permission: Arc<PermissionPolicy>,
    pty: Arc<PTYManager>,
    cc_tasks: Arc<Mutex<CCTasksWatcher>>,
}

fn default_mission_home() -> PathBuf {
    ipc::default_mission_home()
}

fn env_path(var: &str) -> Option<PathBuf> {
    std::env::var(var).ok().map(PathBuf::from)
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

fn db_path() -> PathBuf {
    env_path("MISSION_DB_PATH").unwrap_or_else(|| default_mission_home().join("mission.db"))
}

fn slots_config_path() -> PathBuf {
    env_path("MISSION_SLOTS_CONFIG").unwrap_or_else(|| default_mission_home().join("slots.yaml"))
}

fn ws_port() -> u16 {
    std::env::var("MISSION_WS_PORT")
        .ok()
        .and_then(|v| v.parse::<u16>().ok())
        .unwrap_or(9120)
}

fn logs_dir(db_path: &Path) -> PathBuf {
    db_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("logs")
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

struct PermissionAdapter {
    permission: Arc<PermissionPolicy>,
}

impl missiond_core::PTYPermissionPolicy for PermissionAdapter {
    fn check_permission(
        &self,
        slot_id: &str,
        role: &str,
        tool_name: &str,
    ) -> missiond_core::pty::PermissionDecision {
        match self.permission.check_permission(slot_id, role, tool_name) {
            CorePermissionDecision::Allow => missiond_core::PermissionDecision::Allow,
            CorePermissionDecision::Confirm => missiond_core::PermissionDecision::Confirm,
            CorePermissionDecision::Deny => missiond_core::PermissionDecision::Deny,
        }
    }
}

// =========================
// Tool dispatch (daemon side)
// =========================

#[derive(Deserialize)]
struct SubmitArgs {
    role: String,
    prompt: String,
}

#[derive(Deserialize)]
struct AskArgs {
    role: String,
    question: String,
    #[serde(rename = "timeoutMs", default)]
    timeout_ms: Option<u64>,
}

#[derive(Deserialize)]
struct StatusArgs {
    #[serde(rename = "taskId")]
    task_id: String,
}

#[derive(Deserialize)]
struct CancelArgs {
    #[serde(rename = "taskId")]
    task_id: String,
}

#[derive(Deserialize)]
struct SpawnArgs {
    #[serde(rename = "slotId")]
    slot_id: String,
    #[serde(default)]
    visible: Option<bool>,
    #[serde(rename = "autoRestart", default)]
    auto_restart: Option<bool>,
}

#[derive(Deserialize)]
struct KillArgs {
    #[serde(rename = "slotId")]
    slot_id: String,
}

#[derive(Deserialize)]
struct RestartArgs {
    #[serde(rename = "slotId")]
    slot_id: String,
    #[serde(default)]
    visible: Option<bool>,
}

#[derive(Deserialize)]
struct InboxArgs {
    #[serde(rename = "unreadOnly", default)]
    unread_only: Option<bool>,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Deserialize)]
struct PTYSpawnArgs {
    #[serde(rename = "slotId")]
    slot_id: String,
    #[serde(rename = "autoRestart", default)]
    auto_restart: Option<bool>,
}

#[derive(Deserialize)]
struct PTYSendArgs {
    #[serde(rename = "slotId")]
    slot_id: String,
    message: String,
    #[serde(rename = "timeoutMs", default)]
    timeout_ms: Option<u64>,
}

#[derive(Deserialize)]
struct PTYKillArgs {
    #[serde(rename = "slotId")]
    slot_id: String,
}

#[derive(Deserialize)]
struct PTYScreenArgs {
    #[serde(rename = "slotId")]
    slot_id: String,
    #[serde(default)]
    lines: Option<usize>,
}

#[derive(Deserialize)]
struct PTYHistoryArgs {
    #[serde(rename = "slotId")]
    slot_id: String,
}

#[derive(Deserialize)]
struct PTYStatusArgs {
    #[serde(rename = "slotId")]
    slot_id: Option<String>,
}

#[derive(Deserialize)]
struct PTYConfirmArgs {
    #[serde(rename = "slotId")]
    slot_id: String,
    response: Value,
}

#[derive(Deserialize)]
struct PTYInterruptArgs {
    #[serde(rename = "slotId")]
    slot_id: String,
}

#[derive(Deserialize)]
struct PTYLogsArgs {
    #[serde(rename = "slotId")]
    slot_id: String,
}

#[derive(Deserialize)]
struct SetRolePermissionArgs {
    role: String,
    rule: PermissionRule,
}

#[derive(Deserialize)]
struct SetSlotPermissionArgs {
    #[serde(rename = "slotId")]
    slot_id: String,
    rule: PermissionRule,
}

#[derive(Deserialize)]
struct AddAutoAllowArgs {
    role: Option<String>,
    #[serde(rename = "slotId")]
    slot_id: Option<String>,
    pattern: String,
}

#[derive(Deserialize)]
struct CCSessionsArgs {
    #[serde(rename = "projectPath")]
    project_path: Option<String>,
    #[serde(rename = "activeOnly", default)]
    active_only: Option<bool>,
}

#[derive(Deserialize)]
struct CCTasksArgs {
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    #[serde(rename = "projectPath")]
    project_path: Option<String>,
}

#[derive(Deserialize)]
struct CCTriggerSwarmArgs {
    #[serde(rename = "slotId")]
    slot_id: String,
    tasks: Vec<String>,
    #[serde(rename = "teammateCount", default)]
    teammate_count: Option<usize>,
    #[serde(rename = "timeoutMs", default)]
    timeout_ms: Option<u64>,
}

impl AppState {
    async fn call_tool(&self, name: &str, args: Value) -> ToolResult {
        match self.call_tool_inner(name, args).await {
            Ok(res) => res,
            Err(e) => {
                error!(tool = %name, error = %e, "Tool call failed");
                ToolResult::error(e.to_string())
            }
        }
    }

    async fn call_tool_inner(&self, name: &str, args: Value) -> Result<ToolResult> {
        match name {
            // ===== Task operations =====
            "mission_submit" => {
                let SubmitArgs { role, prompt } = serde_json::from_value(args)?;
                let task_id = self.mission.submit(&role, &prompt)?;
                Ok(ToolResult::json(&serde_json::json!({ "taskId": task_id })))
            }
            "mission_ask" => {
                let AskArgs {
                    role,
                    question,
                    timeout_ms,
                } = serde_json::from_value(args)?;
                let timeout_ms = timeout_ms.unwrap_or(120_000);
                let result = self.mission.ask_expert(&role, &question, timeout_ms).await?;
                Ok(ToolResult::text(result))
            }
            "mission_status" => {
                let StatusArgs { task_id } = serde_json::from_value(args)?;
                if let Some(task) = self.mission.get_status(&task_id) {
                    Ok(ToolResult::json(&task))
                } else {
                    Ok(ToolResult::error("Task not found"))
                }
            }
            "mission_cancel" => {
                let CancelArgs { task_id } = serde_json::from_value(args)?;
                let cancelled = self.mission.cancel(&task_id).await?;
                Ok(ToolResult::json(&serde_json::json!({ "cancelled": cancelled })))
            }

            // ===== Process control =====
            "mission_spawn" => {
                let SpawnArgs {
                    slot_id,
                    visible,
                    auto_restart,
                } = serde_json::from_value(args)?;
                let agent = self
                    .mission
                    .spawn_agent(
                        &slot_id,
                        Some(missiond_core::SpawnOptions {
                            visible: visible.unwrap_or(false),
                            auto_restart: auto_restart.unwrap_or(false),
                        }),
                    )
                    .await?;
                Ok(ToolResult::json(&agent))
            }
            "mission_kill" => {
                let KillArgs { slot_id } = serde_json::from_value(args)?;
                self.mission.kill_agent(&slot_id).await?;
                Ok(ToolResult::json(
                    &serde_json::json!({ "success": true, "slotId": slot_id }),
                ))
            }
            "mission_restart" => {
                let RestartArgs { slot_id, visible } = serde_json::from_value(args)?;
                let agent = self
                    .mission
                    .restart_agent(
                        &slot_id,
                        Some(missiond_core::SpawnOptions {
                            visible: visible.unwrap_or(false),
                            auto_restart: false,
                        }),
                    )
                    .await?;
                Ok(ToolResult::json(&agent))
            }
            "mission_agents" => {
                let agents = self.mission.get_agents();
                Ok(ToolResult::json(&agents))
            }

            // ===== Info =====
            "mission_slots" => Ok(ToolResult::json(&self.mission.list_slots())),
            "mission_inbox" => {
                let InboxArgs {
                    unread_only,
                    limit,
                } = serde_json::from_value(args).unwrap_or(InboxArgs {
                    unread_only: None,
                    limit: None,
                });
                let messages = self
                    .mission
                    .get_inbox(unread_only.unwrap_or(true), limit.unwrap_or(10));
                Ok(ToolResult::json(&messages))
            }

            // ===== PTY =====
            "mission_pty_spawn" => {
                let PTYSpawnArgs {
                    slot_id,
                    auto_restart,
                } = serde_json::from_value(args)?;
                let slot = self
                    .mission
                    .list_slots()
                    .into_iter()
                    .find(|s| s.config.id == slot_id)
                    .ok_or_else(|| anyhow!("Slot not found: {}", slot_id))?;

                let pty_slot = missiond_core::PTYSlot {
                    id: slot.config.id.clone(),
                    role: slot.config.role.clone(),
                    cwd: slot.config.cwd.as_deref().map(PathBuf::from),
                };

                let info = self
                    .pty
                    .spawn(
                        &pty_slot,
                        PTYSpawnOptions {
                            auto_restart: auto_restart.unwrap_or(false),
                        },
                    )
                    .await?;
                Ok(ToolResult::json(&info))
            }
            "mission_pty_send" => {
                let PTYSendArgs {
                    slot_id,
                    message,
                    timeout_ms,
                } = serde_json::from_value(args)?;
                let timeout_ms = timeout_ms.unwrap_or(300_000);
                let res = self.pty.send(&slot_id, &message, timeout_ms).await?;
                Ok(ToolResult::text(res.response))
            }
            "mission_pty_kill" => {
                let PTYKillArgs { slot_id } = serde_json::from_value(args)?;
                self.pty.kill(&slot_id).await?;
                Ok(ToolResult::json(
                    &serde_json::json!({ "success": true, "slotId": slot_id }),
                ))
            }
            "mission_pty_screen" => {
                let PTYScreenArgs { slot_id, lines } = serde_json::from_value(args)?;
                if let Some(n) = lines {
                    let last = self.pty.get_last_lines(&slot_id, n).await?;
                    Ok(ToolResult::text(last.join("\n")))
                } else {
                    Ok(ToolResult::text(self.pty.get_screen(&slot_id).await?))
                }
            }
            "mission_pty_history" => {
                let PTYHistoryArgs { slot_id } = serde_json::from_value(args)?;
                let history = self.pty.get_history(&slot_id).await;
                Ok(ToolResult::json(&history))
            }
            "mission_pty_status" => {
                let PTYStatusArgs { slot_id } = serde_json::from_value(args).unwrap_or(PTYStatusArgs {
                    slot_id: None,
                });
                if let Some(slot_id) = slot_id {
                    let status = self.pty.get_status(&slot_id).await;
                    Ok(ToolResult::json(&status))
                } else {
                    let all = self.pty.get_all_status().await;
                    Ok(ToolResult::json(&all))
                }
            }
            "mission_pty_confirm" => {
                let PTYConfirmArgs { slot_id, response } = serde_json::from_value(args)?;
                let response_echo = response.clone();

                // Map Node-style response (boolean/number/string) to PTY confirm input.
                let resp = match response {
                    Value::Bool(true) => missiond_core::ConfirmResponse::Yes,
                    Value::Bool(false) => missiond_core::ConfirmResponse::No,
                    Value::Number(n) => {
                        let n = n.as_u64().unwrap_or(1) as usize;
                        if n == 1 {
                            missiond_core::ConfirmResponse::Yes
                        } else if n == 3 {
                            missiond_core::ConfirmResponse::No
                        } else {
                            missiond_core::ConfirmResponse::Option(n)
                        }
                    }
                    Value::String(s) => {
                        if s == "y" || s == "Y" || s == "1" {
                            missiond_core::ConfirmResponse::Yes
                        } else if s == "n" || s == "N" || s == "3" {
                            missiond_core::ConfirmResponse::No
                        } else if let Ok(n) = s.parse::<usize>() {
                            if n == 1 {
                                missiond_core::ConfirmResponse::Yes
                            } else if n == 3 {
                                missiond_core::ConfirmResponse::No
                            } else {
                                missiond_core::ConfirmResponse::Option(n)
                            }
                        } else {
                            // Fallback: write raw input + enter
                            let response_text = s.clone();
                            let input = format!("{}\r", s);
                            self.pty.write(&slot_id, &input).await?;
                            return Ok(ToolResult::json(&serde_json::json!({
                                "success": true,
                                "slotId": slot_id,
                                "response": response_text,
                            })));
                        }
                    }
                    _ => missiond_core::ConfirmResponse::Yes,
                };

                self.pty.confirm(&slot_id, resp).await?;
                Ok(ToolResult::json(&serde_json::json!({
                    "success": true,
                    "slotId": slot_id,
                    "response": response_echo,
                })))
            }
            "mission_pty_interrupt" => {
                let PTYInterruptArgs { slot_id } = serde_json::from_value(args)?;
                self.pty.interrupt(&slot_id).await?;
                Ok(ToolResult::json(
                    &serde_json::json!({ "success": true, "slotId": slot_id }),
                ))
            }
            "mission_pty_logs" => {
                let PTYLogsArgs { slot_id } = serde_json::from_value(args)?;
                let status = self.pty.get_status(&slot_id).await;
                let status = status.ok_or_else(|| anyhow!("PTY session not found"))?;
                #[cfg(unix)]
                let hint = format!("tail -f {}", status.log_file.display());
                #[cfg(windows)]
                let hint = format!("Get-Content -Path \"{}\" -Wait -Tail 50", status.log_file.display());

                Ok(ToolResult::json(&serde_json::json!({
                    "slotId": slot_id,
                    "logFile": status.log_file,
                    "hint": hint,
                })))
            }

            // ===== Permission =====
            "mission_permission_get" => Ok(ToolResult::json_pretty(
                &self.permission.get_config(),
            )),
            "mission_permission_set_role" => {
                let SetRolePermissionArgs { role, rule } = serde_json::from_value(args)?;
                self.permission.set_role_rule(&role, rule.clone());
                Ok(ToolResult::json(&serde_json::json!({
                    "success": true,
                    "role": role,
                    "rule": rule,
                })))
            }
            "mission_permission_set_slot" => {
                let SetSlotPermissionArgs { slot_id, rule } = serde_json::from_value(args)?;
                self.permission.set_slot_rule(&slot_id, rule.clone());
                Ok(ToolResult::json(&serde_json::json!({
                    "success": true,
                    "slotId": slot_id,
                    "rule": rule,
                })))
            }
            "mission_permission_add_auto_allow" => {
                let AddAutoAllowArgs {
                    role,
                    slot_id,
                    pattern,
                } = serde_json::from_value(args)?;
                if let Some(role) = role {
                    self.permission.add_role_auto_allow(&role, &pattern);
                    Ok(ToolResult::json(&serde_json::json!({
                        "success": true,
                        "role": role,
                        "pattern": pattern,
                    })))
                } else if let Some(slot_id) = slot_id {
                    self.permission.add_slot_auto_allow(&slot_id, &pattern);
                    Ok(ToolResult::json(&serde_json::json!({
                        "success": true,
                        "slotId": slot_id,
                        "pattern": pattern,
                    })))
                } else {
                    Ok(ToolResult::error("Must specify role or slotId"))
                }
            }
            "mission_permission_reload" => {
                self.permission.reload();
                Ok(ToolResult::json(&serde_json::json!({ "success": true })))
            }

            // ===== Claude Code Tasks =====
            "mission_cc_sessions" => {
                let CCSessionsArgs {
                    project_path,
                    active_only,
                } = serde_json::from_value(args).unwrap_or(CCSessionsArgs {
                    project_path: None,
                    active_only: None,
                });
                let active_only = active_only.unwrap_or(true);

                let sessions = {
                    let cc = self.cc_tasks.lock().await;
                    if active_only {
                        cc.get_active_sessions().await
                    } else {
                        cc.get_all_sessions().await
                    }
                };

                let mut sessions = sessions;
                if let Some(filter) = project_path {
                    sessions = sessions
                        .into_iter()
                        .filter(|s| s.project_path.contains(&filter) || s.project_name.contains(&filter))
                        .collect();
                }

                let result: Vec<Value> = sessions
                    .into_iter()
                    .map(|s| {
                        let mut pending = 0;
                        let mut in_progress = 0;
                        let mut completed = 0;
                        for t in &s.tasks {
                            match t.status {
                                missiond_core::CCTaskStatus::Pending => pending += 1,
                                missiond_core::CCTaskStatus::InProgress => in_progress += 1,
                                missiond_core::CCTaskStatus::Completed => completed += 1,
                            }
                        }

                        serde_json::json!({
                            "sessionId": s.session_id,
                            "project": s.project_name,
                            "summary": s.summary,
                            "tasks": s.tasks.len(),
                            "inProgress": in_progress,
                            "pending": pending,
                            "completed": completed,
                            "modified": s.modified,
                            "isActive": s.is_active,
                        })
                    })
                    .collect();

                Ok(ToolResult::json_pretty(&result))
            }
            "mission_cc_tasks" => {
                let CCTasksArgs {
                    session_id,
                    project_path,
                } = serde_json::from_value(args).unwrap_or(CCTasksArgs {
                    session_id: None,
                    project_path: None,
                });

                if let Some(session_id) = session_id {
                    let tasks = {
                        let cc = self.cc_tasks.lock().await;
                        cc.get_session_tasks(&session_id).await
                    };
                    if let Some(tasks) = tasks {
                        return Ok(ToolResult::json_pretty(&tasks));
                    }
                    return Ok(ToolResult::error("Session not found"));
                }

                if let Some(project_path) = project_path {
                    let sessions = {
                        let cc = self.cc_tasks.lock().await;
                        cc.get_sessions_by_project(&project_path).await
                    };
                    let result: Vec<Value> = sessions
                        .into_iter()
                        .map(|s| {
                            serde_json::json!({
                                "sessionId": s.session_id,
                                "summary": s.summary,
                                "tasks": s.tasks,
                            })
                        })
                        .collect();
                    return Ok(ToolResult::json_pretty(&result));
                }

                Ok(ToolResult::error("Provide sessionId or projectPath"))
            }
            "mission_cc_overview" => {
                let overview = { self.cc_tasks.lock().await.get_overview().await };
                Ok(ToolResult::json_pretty(&overview))
            }
            "mission_cc_in_progress" => {
                let in_progress = { self.cc_tasks.lock().await.get_in_progress_tasks().await };
                let result: Vec<Value> = in_progress
                    .into_iter()
                    .map(|item| {
                        serde_json::json!({
                            "sessionId": item.session_id,
                            "project": item.project_name,
                            "summary": item.summary,
                            "task": item.task.content,
                            "activeForm": item.task.active_form,
                            "modified": item.modified,
                        })
                    })
                    .collect();
                Ok(ToolResult::json_pretty(&result))
            }
            "mission_cc_trigger_swarm" => {
                let CCTriggerSwarmArgs {
                    slot_id,
                    tasks,
                    teammate_count,
                    timeout_ms,
                } = serde_json::from_value(args)?;
                let teammate_count = teammate_count.unwrap_or(3);
                let timeout_ms = timeout_ms.unwrap_or(600_000);

                let prompt = format!(
                    "请进入 Plan 模式，创建以下任务，然后用 {} 个 teammate 并行执行：\n\n{}\n\n完成后汇报结果。",
                    teammate_count,
                    tasks
                        .iter()
                        .enumerate()
                        .map(|(i, t)| format!("{}. {}", i + 1, t))
                        .collect::<Vec<_>>()
                        .join("\n")
                );

                let res = self.pty.send(&slot_id, &prompt, timeout_ms).await?;
                Ok(ToolResult::text(res.response))
            }

            _ => {
                let mut res = ToolResult::text(format!("Unknown tool: {}", name));
                res.is_error = Some(true);
                Ok(res)
            }
        }
    }
}

// =========================
// IPC server (daemon)
// =========================

async fn handle_ipc_connection(state: AppState, mut reader: BufReader<IpcStream>) -> Result<()> {
    let mut line = String::new();
    let bytes = reader.read_line(&mut line).await?;
    if bytes == 0 {
        return Ok(());
    }

    let message = line.trim();
    let request = match protocol::parse_request_str(message) {
        Ok(r) => r,
        Err(e) => {
            let resp = Response::from_error(RequestId::Null, e);
            let json = protocol::serialize_response_string(&resp)?;
            reader.get_mut().write_all(json.as_bytes()).await?;
            reader.get_mut().write_all(b"\n").await?;
            reader.get_mut().flush().await?;
            return Ok(());
        }
    };

    let resp = handle_ipc_request(state, request).await;
    let json = protocol::serialize_response_string(&resp)?;
    reader.get_mut().write_all(json.as_bytes()).await?;
    reader.get_mut().write_all(b"\n").await?;
    reader.get_mut().flush().await?;
    Ok(())
}

async fn handle_ipc_request(state: AppState, request: Request) -> Response {
    let id = request.id.clone();
    let method = request.method.as_str();
    let params = request.params.unwrap_or(Value::Null);

    match method {
        "ping" => Response::success(id, serde_json::json!({})),
        "tools/call" => {
            let name = match params.get("name").and_then(|v| v.as_str()) {
                Some(n) => n.to_string(),
                None => {
                    return Response::from_error(
                        id,
                        RpcError::InvalidParams("Missing 'name' field".to_string()),
                    );
                }
            };
            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or(Value::Object(serde_json::Map::new()));

            let tool_res = state.call_tool(&name, arguments).await;
            Response::success(id, serde_json::to_value(tool_res).unwrap_or(Value::Null))
        }
        _ => Response::from_error(id, RpcError::MethodNotFound(method.to_string())),
    }
}

async fn bind_ipc_listener(endpoint: &str) -> Result<IpcListener> {
    IpcListener::bind(endpoint).await
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(log_filter())
        .with_writer(std::io::stderr)
        .init();

    let home = default_mission_home();
    std::fs::create_dir_all(&home).ok();

    let db_path = db_path();
    let slots_path = slots_config_path();
    if !slots_path.exists() {
        return Err(anyhow!(
            "Slots config not found: {} (set MISSION_SLOTS_CONFIG or create slots.yaml)",
            slots_path.display()
        ));
    }

    let logs_dir = logs_dir(&db_path);
    let permission_config_path = db_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("config")
        .join("permissions.yaml");
    let permission = Arc::new(PermissionPolicy::new(&permission_config_path));

    let mission = Arc::new(MissionControl::new(MissionControlOptions {
        db_path: db_path.clone(),
        slots_config_path: slots_path.clone(),
        permission_config_path: None,
        logs_dir: Some(logs_dir.clone()),
        default_mode: None,
    })?);
    mission.start().await?;

    // PTY manager setup
    let pty = Arc::new(PTYManager::new(logs_dir.clone()));
    pty.set_permission_policy(Arc::new(PermissionAdapter {
        permission: Arc::clone(&permission),
    }))
    .await;

    // Init PTY slots
    for slot in mission.list_slots() {
        let pty_slot = missiond_core::PTYSlot {
            id: slot.config.id.clone(),
            role: slot.config.role.clone(),
            cwd: slot.config.cwd.as_deref().map(PathBuf::from),
        };
        pty.init_slot(&pty_slot).await;
    }

    // CC tasks watcher
    let mut cc = CCTasksWatcher::new(CCTasksWatcherOptions::default());
    cc.start().await?;
    let cc_tasks = Arc::new(Mutex::new(cc));

    // WebSocket server (PTY attach + Tasks events)
    let ws_port = ws_port();
    let mut ws_server = PTYWebSocketServer::new(WSServerOptions {
        port: ws_port,
        pty_manager: Some(Arc::clone(&pty)),
        cc_tasks_watcher: Some(Arc::clone(&cc_tasks)),
    });
    if let Err(e) = ws_server.start().await {
        // Match Node behavior: continue running even if WS is unavailable (e.g. port in use).
        warn!(port = ws_port, error = %e, "Failed to start WebSocket server");
    }

    // IPC server
    let endpoint = ipc_endpoint_from_env();
    let listener = bind_ipc_listener(&endpoint).await?;
    info!(endpoint = %endpoint, "missiond IPC listening");

    let state = AppState {
        mission,
        permission,
        pty,
        cc_tasks,
    };

    loop {
        let stream = listener.accept().await?;
        let reader = BufReader::new(stream);
        if let Err(e) = handle_ipc_connection(state.clone(), reader).await {
            warn!(error = %e, "IPC connection error");
        }
    }
}
