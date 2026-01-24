//! Process Manager - Claude Code agent process lifecycle management
//!
//! Manages the lifecycle of Claude Code agent child processes.

use crate::db::MissionDB;
use crate::types::{Slot, Task};
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, RwLock};
use tokio::fs::OpenOptions;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::broadcast;
use tracing::{debug, error, info, warn};

/// Agent status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentStatus {
    Stopped,
    Starting,
    Idle,
    Busy,
    Stopping,
}

impl AgentStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            AgentStatus::Stopped => "stopped",
            AgentStatus::Starting => "starting",
            AgentStatus::Idle => "idle",
            AgentStatus::Busy => "busy",
            AgentStatus::Stopping => "stopping",
        }
    }
}

/// Agent process information
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentProcess {
    pub slot_id: String,
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    pub status: AgentStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_task_id: Option<String>,
    pub log_file: PathBuf,
}

/// Options for spawning an agent
#[derive(Debug, Clone, Default)]
pub struct SpawnOptions {
    /// Open a visible terminal window
    pub visible: bool,
    /// Auto-restart on crash
    pub auto_restart: bool,
}

/// Result of executing a task
#[derive(Debug, Clone)]
pub struct ExecuteResult {
    pub result: String,
    pub session_id: String,
}

/// Process Manager event
#[derive(Debug, Clone)]
pub enum ProcessEvent {
    AgentSpawned(String),
    AgentKilled(String),
    AgentBusy(String),
    AgentIdle(String),
}

/// Process Manager
///
/// Manages Claude Code Agent child process lifecycle
pub struct ProcessManager {
    processes: Arc<RwLock<HashMap<String, AgentProcess>>>,
    child_processes: Arc<RwLock<HashMap<String, Child>>>,
    db: Arc<MissionDB>,
    logs_dir: PathBuf,
    auto_restart_slots: Arc<RwLock<HashSet<String>>>,
    event_tx: broadcast::Sender<ProcessEvent>,
}

impl ProcessManager {
    /// Create a new ProcessManager
    pub fn new(db: Arc<MissionDB>, logs_dir: PathBuf) -> Self {
        // Ensure logs directory exists
        std::fs::create_dir_all(&logs_dir).ok();

        let (event_tx, _) = broadcast::channel(100);

        Self {
            processes: Arc::new(RwLock::new(HashMap::new())),
            child_processes: Arc::new(RwLock::new(HashMap::new())),
            db,
            logs_dir,
            auto_restart_slots: Arc::new(RwLock::new(HashSet::new())),
            event_tx,
        }
    }

    /// Get event receiver
    pub fn subscribe(&self) -> broadcast::Receiver<ProcessEvent> {
        self.event_tx.subscribe()
    }

    /// Initialize a slot's process state
    pub fn init_slot(&self, slot: &Slot) {
        let log_file = self.logs_dir.join(format!("{}.log", slot.config.id));
        let session_id = self.db.get_slot_session(&slot.config.id).ok().flatten();

        let agent = AgentProcess {
            slot_id: slot.config.id.clone(),
            role: slot.config.role.clone(),
            pid: None,
            status: AgentStatus::Stopped,
            session_id,
            started_at: None,
            current_task_id: None,
            log_file,
        };

        let mut processes = self.processes.write().unwrap();
        processes.insert(slot.config.id.clone(), agent);

        debug!(slot_id = %slot.config.id, role = %slot.config.role, "Slot initialized");
    }

    /// Spawn an agent process
    pub async fn spawn(&self, slot: &Slot, options: SpawnOptions) -> Result<AgentProcess> {
        let slot_id = &slot.config.id;

        // Get current agent state
        {
            let processes = self.processes.read().unwrap();
            let agent = processes
                .get(slot_id)
                .ok_or_else(|| anyhow!("Slot not initialized: {}", slot_id))?;

            if agent.status != AgentStatus::Stopped {
                return Err(anyhow!(
                    "Agent already running: {} (status: {:?})",
                    slot_id,
                    agent.status
                ));
            }
        }

        // Update to starting
        {
            let mut processes = self.processes.write().unwrap();
            if let Some(agent) = processes.get_mut(slot_id) {
                agent.status = AgentStatus::Starting;
            }
        }

        if options.auto_restart {
            let mut auto_restart = self.auto_restart_slots.write().unwrap();
            auto_restart.insert(slot_id.clone());
        }

        // Spawn based on mode
        if options.visible {
            self.spawn_visible(slot).await?;
        } else {
            self.spawn_headless(slot).await?;
        }

        // Update to idle
        let agent = {
            let mut processes = self.processes.write().unwrap();
            if let Some(agent) = processes.get_mut(slot_id) {
                agent.status = AgentStatus::Idle;
                agent.started_at = Some(chrono::Utc::now().timestamp_millis());
                info!(slot_id = %slot_id, pid = ?agent.pid, "Agent spawned");
                agent.clone()
            } else {
                return Err(anyhow!("Agent not found after spawn: {}", slot_id));
            }
        };

        let _ = self.event_tx.send(ProcessEvent::AgentSpawned(slot_id.clone()));

        Ok(agent)
    }

    /// Spawn in headless mode
    async fn spawn_headless(&self, slot: &Slot) -> Result<()> {
        // In headless mode, we don't start a persistent process.
        // Instead, we run `claude -p` on demand when executing tasks.
        debug!(slot_id = %slot.config.id, "Headless mode: ready for tasks");
        Ok(())
    }

    /// Spawn in visible mode (open terminal window)
    async fn spawn_visible(&self, slot: &Slot) -> Result<()> {
        let default_cwd = std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| ".".to_string());
        let cwd = slot.config.cwd.as_deref().unwrap_or(&default_cwd);

        #[cfg(target_os = "macos")]
        {
            // macOS: Use osascript to open Terminal
            let script = format!(
                r#"
                tell application "Terminal"
                    do script "cd {} && echo 'Agent {} ready ({})' && read -p 'Press Enter to exit...'"
                    activate
                end tell
            "#,
                cwd, slot.config.id, slot.config.role
            );

            let mut child = Command::new("osascript")
                .arg("-e")
                .arg(&script)
                .spawn()?;

            child.wait().await?;
        }

        #[cfg(target_os = "windows")]
        {
            // Windows: Open new cmd window
            let cmd = format!(
                "start cmd /k \"cd /d {} && echo Agent {} ready ({}) && pause\"",
                cwd, slot.config.id, slot.config.role
            );

            let mut child = Command::new("cmd")
                .args(["/C", &cmd])
                .spawn()?;

            child.wait().await?;
        }

        #[cfg(target_os = "linux")]
        {
            // Linux: Try common terminal emulators
            let msg = format!("Agent {} ready ({})", slot.config.id, slot.config.role);

            // Try gnome-terminal first, then xterm
            let result = Command::new("gnome-terminal")
                .args(["--working-directory", cwd, "--", "bash", "-c", &format!("echo '{}'; read -p 'Press Enter to exit...'", msg)])
                .spawn();

            if result.is_err() {
                // Fallback to xterm
                let mut child = Command::new("xterm")
                    .args(["-e", &format!("cd {} && echo '{}' && read -p 'Press Enter to exit...'", cwd, msg)])
                    .spawn()?;
                child.wait().await?;
            }
        }

        info!(slot_id = %slot.config.id, "Visible terminal opened");
        Ok(())
    }

    /// Execute a task
    pub async fn execute_task(&self, slot: &Slot, task: &Task) -> Result<ExecuteResult> {
        let slot_id = &slot.config.id;

        // Check agent state
        let (session_id, log_file) = {
            let mut processes = self.processes.write().unwrap();
            let agent = processes
                .get_mut(slot_id)
                .ok_or_else(|| anyhow!("Slot not initialized: {}", slot_id))?;

            if agent.status == AgentStatus::Stopped {
                return Err(anyhow!("Agent not running: {}", slot_id));
            }

            if agent.status == AgentStatus::Busy {
                return Err(anyhow!("Agent is busy: {}", slot_id));
            }

            // Mark as busy
            agent.status = AgentStatus::Busy;
            agent.current_task_id = Some(task.id.clone());

            (agent.session_id.clone(), agent.log_file.clone())
        };

        let _ = self.event_tx.send(ProcessEvent::AgentBusy(slot_id.clone()));

        // Open log file
        let mut log_file_handle = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_file)
            .await?;

        let now = chrono::Utc::now().to_rfc3339();
        log_file_handle
            .write_all(format!("\n--- Task {} started at {} ---\n", task.id, now).as_bytes())
            .await?;
        log_file_handle
            .write_all(format!("Prompt: {}\n\n", task.prompt).as_bytes())
            .await?;

        // Run claude command
        let result = self
            .run_claude_command(slot, task, session_id.as_deref(), &mut log_file_handle)
            .await;

        // Update agent state
        {
            let mut processes = self.processes.write().unwrap();
            if let Some(agent) = processes.get_mut(slot_id) {
                agent.status = AgentStatus::Idle;
                agent.current_task_id = None;

                if let Ok(ref res) = result {
                    agent.session_id = Some(res.session_id.clone());
                    let _ = self.db.set_slot_session(slot_id, &res.session_id);
                }
            }
        }

        let _ = self.event_tx.send(ProcessEvent::AgentIdle(slot_id.clone()));

        // Write completion log
        if let Ok(ref res) = result {
            log_file_handle
                .write_all(format!("\n--- Task {} completed ---\n", task.id).as_bytes())
                .await?;
            let preview = if res.result.len() > 500 {
                format!("{}...", &res.result[..500])
            } else {
                res.result.clone()
            };
            log_file_handle
                .write_all(format!("Result: {}\n", preview).as_bytes())
                .await?;
        }

        result
    }

    /// Run the Claude command
    async fn run_claude_command(
        &self,
        slot: &Slot,
        task: &Task,
        session_id: Option<&str>,
        log_file: &mut tokio::fs::File,
    ) -> Result<ExecuteResult> {
        let slot_id = &slot.config.id;
        let cwd = slot
            .config
            .cwd
            .as_deref()
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::current_dir().unwrap());

        let mut args = vec![
            "-p".to_string(),
            task.prompt.clone(),
            "--output-format".to_string(),
            "stream-json".to_string(),
            "--verbose".to_string(),
        ];

        if let Some(sid) = session_id {
            args.push("--resume".to_string());
            args.push(sid.to_string());
        }

        debug!(slot_id = %slot_id, task_id = %task.id, cwd = ?cwd, "Running claude command");

        let child = Command::new("claude")
            .args(&args)
            .current_dir(&cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        let pid = child.id();

        // Store child process
        {
            let mut children = self.child_processes.write().unwrap();
            children.insert(slot_id.clone(), child);
        }

        // Take references to stdout/stderr
        let child = {
            let mut children = self.child_processes.write().unwrap();
            children.remove(slot_id)
        };

        let mut child = child.ok_or_else(|| anyhow!("Child process not found"))?;

        let stdout = child.stdout.take().ok_or_else(|| anyhow!("No stdout"))?;
        let stderr = child.stderr.take().ok_or_else(|| anyhow!("No stderr"))?;

        // Update PID
        {
            let mut processes = self.processes.write().unwrap();
            if let Some(agent) = processes.get_mut(slot_id) {
                agent.pid = pid;
            }
        }

        // Read stdout
        let mut stdout_reader = BufReader::new(stdout).lines();
        let mut result_text = String::new();
        let mut final_session_id = session_id.map(String::from).unwrap_or_default();

        // Spawn stderr reader
        let slot_id_clone = slot_id.clone();
        let stderr_handle = tokio::spawn(async move {
            let mut stderr_reader = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = stderr_reader.next_line().await {
                warn!(slot_id = %slot_id_clone, "[stderr] {}", line);
            }
        });

        // Read stdout lines
        while let Ok(Some(line)) = stdout_reader.next_line().await {
            log_file.write_all(format!("{}\n", line).as_bytes()).await?;

            // Parse stream-json events
            if let Ok(event) = serde_json::from_str::<serde_json::Value>(&line) {
                if let Some(event_type) = event.get("type").and_then(|t| t.as_str()) {
                    match event_type {
                        "result" => {
                            if let Some(r) = event.get("result").and_then(|r| r.as_str()) {
                                result_text = r.to_string();
                            }
                            if let Some(sid) = event.get("session_id").and_then(|s| s.as_str()) {
                                final_session_id = sid.to_string();
                            }
                        }
                        "system" => {
                            if let Some(sid) = event.get("session_id").and_then(|s| s.as_str()) {
                                final_session_id = sid.to_string();
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        // Wait for process
        let status = child.wait().await?;

        // Wait for stderr reader
        let _ = stderr_handle.await;

        // Clear PID
        {
            let mut processes = self.processes.write().unwrap();
            if let Some(agent) = processes.get_mut(slot_id) {
                agent.pid = None;
            }
        }

        if status.success() {
            Ok(ExecuteResult {
                result: result_text,
                session_id: final_session_id,
            })
        } else {
            Err(anyhow!(
                "Claude exited with code {}",
                status.code().unwrap_or(-1)
            ))
        }
    }

    /// Kill an agent
    pub async fn kill(&self, slot_id: &str) -> Result<()> {
        // Check if agent exists
        {
            let processes = self.processes.read().unwrap();
            let agent = processes
                .get(slot_id)
                .ok_or_else(|| anyhow!("Slot not found: {}", slot_id))?;

            if agent.status == AgentStatus::Stopped {
                return Ok(());
            }
        }

        // Update to stopping
        {
            let mut processes = self.processes.write().unwrap();
            if let Some(agent) = processes.get_mut(slot_id) {
                agent.status = AgentStatus::Stopping;
            }
        }

        // Remove from auto-restart
        {
            let mut auto_restart = self.auto_restart_slots.write().unwrap();
            auto_restart.remove(slot_id);
        }

        // Kill child process if running
        {
            let mut children = self.child_processes.write().unwrap();
            if let Some(mut child) = children.remove(slot_id) {
                child.kill().await.ok();
            }
        }

        // Update to stopped
        {
            let mut processes = self.processes.write().unwrap();
            if let Some(agent) = processes.get_mut(slot_id) {
                agent.status = AgentStatus::Stopped;
                agent.pid = None;
                agent.current_task_id = None;
            }
        }

        info!(slot_id = %slot_id, "Agent killed");
        let _ = self.event_tx.send(ProcessEvent::AgentKilled(slot_id.to_string()));

        Ok(())
    }

    /// Restart an agent
    pub async fn restart(&self, slot: &Slot, options: SpawnOptions) -> Result<AgentProcess> {
        self.kill(&slot.config.id).await?;
        self.spawn(slot, options).await
    }

    /// Get agent status
    pub fn get_status(&self, slot_id: &str) -> Option<AgentProcess> {
        let processes = self.processes.read().unwrap();
        processes.get(slot_id).cloned()
    }

    /// Get all agent statuses
    pub fn get_all_status(&self) -> Vec<AgentProcess> {
        let processes = self.processes.read().unwrap();
        processes.values().cloned().collect()
    }

    /// Check if agent is available (idle)
    pub fn is_available(&self, slot_id: &str) -> bool {
        let processes = self.processes.read().unwrap();
        processes
            .get(slot_id)
            .map(|a| a.status == AgentStatus::Idle)
            .unwrap_or(false)
    }

    /// Check if agent is running
    pub fn is_running(&self, slot_id: &str) -> bool {
        let processes = self.processes.read().unwrap();
        processes
            .get(slot_id)
            .map(|a| a.status == AgentStatus::Idle || a.status == AgentStatus::Busy)
            .unwrap_or(false)
    }

    /// Get statistics
    pub fn get_stats(&self) -> ProcessStats {
        let processes = self.processes.read().unwrap();

        let mut stopped = 0;
        let mut idle = 0;
        let mut busy = 0;

        for agent in processes.values() {
            match agent.status {
                AgentStatus::Stopped | AgentStatus::Stopping => stopped += 1,
                AgentStatus::Idle | AgentStatus::Starting => idle += 1,
                AgentStatus::Busy => busy += 1,
            }
        }

        ProcessStats {
            total: processes.len(),
            stopped,
            idle,
            busy,
        }
    }

    /// Shutdown all agents
    pub async fn shutdown(&self) {
        info!("Shutting down all agents...");

        let slot_ids: Vec<String> = {
            let processes = self.processes.read().unwrap();
            processes.keys().cloned().collect()
        };

        for slot_id in slot_ids {
            if let Err(e) = self.kill(&slot_id).await {
                error!(slot_id = %slot_id, error = %e, "Error killing agent");
            }
        }

        info!("All agents shut down");
    }
}

/// Process statistics
#[derive(Debug, Clone)]
pub struct ProcessStats {
    pub total: usize,
    pub stopped: usize,
    pub idle: usize,
    pub busy: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::SlotConfig;
    use tempfile::tempdir;

    fn create_test_db() -> Arc<MissionDB> {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        Arc::new(MissionDB::open(db_path).unwrap())
    }

    fn create_test_slot() -> Slot {
        Slot {
            config: SlotConfig {
                id: "test-slot".to_string(),
                role: "worker".to_string(),
                description: "Test slot".to_string(),
                cwd: None,
                mcp_config: None,
                auto_start: None,
            },
            session_id: None,
        }
    }

    #[tokio::test]
    async fn test_init_slot() {
        let db = create_test_db();
        let logs_dir = tempdir().unwrap().path().to_path_buf();
        let manager = ProcessManager::new(db, logs_dir);

        let slot = create_test_slot();
        manager.init_slot(&slot);

        let status = manager.get_status("test-slot").unwrap();
        assert_eq!(status.slot_id, "test-slot");
        assert_eq!(status.status, AgentStatus::Stopped);
    }

    #[tokio::test]
    async fn test_get_stats() {
        let db = create_test_db();
        let logs_dir = tempdir().unwrap().path().to_path_buf();
        let manager = ProcessManager::new(db, logs_dir);

        let slot1 = Slot {
            config: SlotConfig {
                id: "slot-1".to_string(),
                role: "worker".to_string(),
                description: "Slot 1".to_string(),
                cwd: None,
                mcp_config: None,
                auto_start: None,
            },
            session_id: None,
        };

        let slot2 = Slot {
            config: SlotConfig {
                id: "slot-2".to_string(),
                role: "worker".to_string(),
                description: "Slot 2".to_string(),
                cwd: None,
                mcp_config: None,
                auto_start: None,
            },
            session_id: None,
        };

        manager.init_slot(&slot1);
        manager.init_slot(&slot2);

        let stats = manager.get_stats();
        assert_eq!(stats.total, 2);
        assert_eq!(stats.stopped, 2);
        assert_eq!(stats.idle, 0);
        assert_eq!(stats.busy, 0);
    }
}
