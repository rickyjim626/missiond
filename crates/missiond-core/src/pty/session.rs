//! PTY Session - Interactive terminal session for Claude Code
//!
//! Architecture: portable-pty (process) + alacritty_terminal (emulation) + semantic (detection)
//!
//! - portable-pty: Handles low-level PTY process communication
//! - alacritty_terminal: Parses ANSI sequences, maintains virtual screen
//! - semantic: State detection and confirmation dialog parsing

use std::collections::HashMap;
use std::io::{Read, Write as IoWrite};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use alacritty_terminal::event::{Event as TermEvent, EventListener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::{Config as TermConfig, Term};
use anyhow::{anyhow, Result};

/// Terminal size for creating Term
struct TermSize {
    cols: usize,
    rows: usize,
}

impl Dimensions for TermSize {
    fn total_lines(&self) -> usize {
        self.rows
    }

    fn screen_lines(&self) -> usize {
        self.rows
    }

    fn columns(&self) -> usize {
        self.cols
    }
}
use chrono::Utc;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc, oneshot, Mutex, RwLock};
use tokio::time::{interval, timeout};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use super::extractor::{IncrementalExtractor, StableTextOp, TextAssembler};
use crate::semantic::{
    ClaudeCodeConfirmParser, ClaudeCodeStateParser, ClaudeCodeStatusParser,
    ClaudeCodeToolOutputParser,
    ConfirmParser, ParserContext, StateParser, StatusParser, ToolOutputParser,
    default_registry,
    ClaudeCodeStatus, ClaudeCodeToolOutput, ClaudeCodeTitle,
    ConfirmInfo as SemanticConfirmInfo,
    State as SemanticState,
};

// ========== Types ==========

/// Session state machine
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    /// Starting up
    Starting,
    /// Waiting for input (shows >)
    Idle,
    /// Claude is thinking (shows spinner)
    Thinking,
    /// Claude is outputting response
    Responding,
    /// Tool is executing
    ToolRunning,
    /// Waiting for confirmation (Y/n)
    Confirming,
    /// Error state
    Error,
    /// Session has exited
    Exited,
}

impl SessionState {
    /// Check if this is a processing state (Claude is active)
    pub fn is_processing(&self) -> bool {
        matches!(
            self,
            SessionState::Thinking | SessionState::ToolRunning | SessionState::Responding
        )
    }
}

/// Chat message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: MessageRole,
    pub content: String,
    pub timestamp: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    User,
    Assistant,
}

/// Source of screen text
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScreenTextSource {
    Assistant,
    User,
    Tool,
    Ui,
    Unknown,
}

/// Text output event (streaming or complete)
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TextOutputEvent {
    Stream {
        turn_id: u64,
        seq: u64,
        content: String,
        timestamp: i64,
    },
    Complete {
        turn_id: u64,
        content: String,
        timestamp: i64,
    },
}

/// Screen text event for non-assistant content
#[derive(Debug, Clone, Serialize)]
pub struct ScreenTextEvent {
    pub source: ScreenTextSource,
    pub kind: String,
    pub y: usize,
    pub content: String,
    pub timestamp: i64,
    pub turn_id: Option<u64>,
}

/// Tool information from confirmation dialog
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolInfo {
    pub name: String,
    pub mcp_server: Option<String>,
    pub params: HashMap<String, serde_json::Value>,
}

/// Confirmation dialog information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfirmInfo {
    #[serde(rename = "type")]
    pub confirm_type: String,
    pub tool: Option<ToolInfo>,
    pub options: Vec<String>,
    pub selected: usize,
}

/// Permission decision for tool execution
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionDecision {
    /// Auto-approve the tool
    Allow,
    /// Auto-deny the tool
    Deny,
    /// Require manual confirmation
    Confirm,
}

/// PTY session options
#[derive(Debug, Clone)]
pub struct PTYSessionOptions {
    pub slot_id: String,
    pub cwd: PathBuf,
    pub env: Option<HashMap<String, String>>,
    pub log_file: Option<PathBuf>,
    pub cols: u16,
    pub rows: u16,
}

impl Default for PTYSessionOptions {
    fn default() -> Self {
        Self {
            slot_id: Uuid::new_v4().to_string(),
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")),
            env: None,
            log_file: None,
            cols: 120,
            rows: 30,
        }
    }
}

/// Event listener for alacritty terminal
struct SessionEventListener {
    sender: mpsc::UnboundedSender<TermEvent>,
}

impl EventListener for SessionEventListener {
    fn send_event(&self, event: TermEvent) {
        let _ = self.sender.send(event);
    }
}

// ========== PTYSession ==========

/// Interactive PTY session for Claude Code
///
/// Manages a single Claude Code process with terminal emulation,
/// state detection, and streaming text extraction.
pub struct PTYSession {
    /// Unique session ID
    pub id: String,
    /// Slot ID this session belongs to
    pub slot_id: String,
    /// Working directory
    pub cwd: PathBuf,
    /// Terminal dimensions
    pub cols: u16,
    pub rows: u16,

    // Internal state
    state: Arc<RwLock<SessionState>>,
    history: Arc<RwLock<Vec<Message>>>,
    terminal_title: Arc<RwLock<String>>,
    pending_tool_confirm: Arc<RwLock<Option<ConfirmInfo>>>,
    permission_check: Arc<RwLock<Option<Box<dyn Fn(&ConfirmInfo) -> PermissionDecision + Send + Sync>>>>,

    // PTY process
    pty_writer: Arc<Mutex<Option<Box<dyn IoWrite + Send>>>>,
    pty_pid: Arc<RwLock<Option<u32>>>,
    running: Arc<AtomicBool>,

    // Terminal emulation
    term: Arc<Mutex<Term<SessionEventListener>>>,

    // Text extraction
    extractor: Arc<Mutex<IncrementalExtractor>>,
    text_assembler: Arc<Mutex<TextAssembler>>,
    current_turn_id: Arc<RwLock<Option<u64>>>,
    stream_seq: Arc<RwLock<u64>>,
    turn_counter: Arc<RwLock<u64>>,
    line_source_by_y: Arc<RwLock<HashMap<usize, ScreenTextSource>>>,
    assistant_block_active: Arc<AtomicBool>,

    // Event channels
    event_tx: broadcast::Sender<SessionEvent>,
    state_change_tx: broadcast::Sender<(SessionState, SessionState)>,
    shutdown_tx: Option<oneshot::Sender<()>>,

    // Logging
    log_file: Option<PathBuf>,
}

/// Events emitted by the session
#[derive(Debug, Clone)]
pub enum SessionEvent {
    /// Raw data from PTY
    Data(Vec<u8>),
    /// State changed
    StateChange {
        new_state: SessionState,
        prev_state: SessionState,
    },
    /// Text output (stream or complete)
    TextOutput(TextOutputEvent),
    /// Screen text (non-assistant)
    ScreenText(ScreenTextEvent),
    /// Confirmation required
    ConfirmRequired {
        prompt: String,
        info: Option<ConfirmInfo>,
    },
    /// Status bar update (spinner + status text)
    StatusUpdate(ClaudeCodeStatus),
    /// Tool output parsed
    ToolOutput(ClaudeCodeToolOutput),
    /// Terminal title changed
    TitleChange(ClaudeCodeTitle),
    /// Session exited
    Exit(i32),
}

impl PTYSession {
    /// Create a new PTY session
    pub fn new(options: PTYSessionOptions) -> Result<Self> {
        let id = format!(
            "pty-{}-{}",
            Utc::now().timestamp_millis(),
            &Uuid::new_v4().to_string()[..8]
        );

        // Create terminal event channel
        let (term_event_tx, _term_event_rx) = mpsc::unbounded_channel();
        let event_listener = SessionEventListener {
            sender: term_event_tx,
        };

        // Create virtual terminal
        let term_config = TermConfig::default();
        let term_size = TermSize {
            cols: options.cols as usize,
            rows: options.rows as usize,
        };
        let term = Term::new(term_config, &term_size, event_listener);

        // Create event channels
        let (event_tx, _) = broadcast::channel(1000);
        let (state_change_tx, _) = broadcast::channel(100);

        Ok(Self {
            id,
            slot_id: options.slot_id,
            cwd: options.cwd,
            cols: options.cols,
            rows: options.rows,

            state: Arc::new(RwLock::new(SessionState::Starting)),
            history: Arc::new(RwLock::new(Vec::new())),
            terminal_title: Arc::new(RwLock::new(String::new())),
            pending_tool_confirm: Arc::new(RwLock::new(None)),
            permission_check: Arc::new(RwLock::new(None)),

            pty_writer: Arc::new(Mutex::new(None)),
            pty_pid: Arc::new(RwLock::new(None)),
            running: Arc::new(AtomicBool::new(false)),

            term: Arc::new(Mutex::new(term)),
            extractor: Arc::new(Mutex::new(IncrementalExtractor::new(
                options.rows as usize,
                None,
            ))),
            text_assembler: Arc::new(Mutex::new(TextAssembler::new())),
            current_turn_id: Arc::new(RwLock::new(None)),
            stream_seq: Arc::new(RwLock::new(0)),
            turn_counter: Arc::new(RwLock::new(0)),
            line_source_by_y: Arc::new(RwLock::new(HashMap::new())),
            assistant_block_active: Arc::new(AtomicBool::new(false)),

            event_tx,
            state_change_tx,
            shutdown_tx: None,
            log_file: options.log_file,
        })
    }

    // ========== Getters ==========

    /// Get current state
    pub async fn state(&self) -> SessionState {
        *self.state.read().await
    }

    /// Get chat history
    pub async fn history(&self) -> Vec<Message> {
        self.history.read().await.clone()
    }

    /// Check if session is running
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Get process ID
    pub async fn pid(&self) -> Option<u32> {
        *self.pty_pid.read().await
    }

    /// Get pending tool confirmation
    pub async fn pending_tool_confirm(&self) -> Option<ConfirmInfo> {
        self.pending_tool_confirm.read().await.clone()
    }

    /// Get terminal title
    pub async fn terminal_title(&self) -> String {
        self.terminal_title.read().await.clone()
    }

    // ========== Screen Reading ==========

    /// Get current screen text
    pub async fn get_screen_text(&self) -> String {
        let term = self.term.lock().await;
        let grid = term.grid();
        let mut lines = Vec::new();

        let total_lines = grid.total_lines();
        let rows = grid.screen_lines();
        let start = if total_lines > rows {
            total_lines - rows
        } else {
            0
        };

        for y in start..total_lines {
            let line = alacritty_terminal::index::Line(y as i32);
            if y < grid.total_lines() {
                let row = &grid[line];
                let text: String = row.into_iter().map(|cell| cell.c).collect();
                lines.push(text.trim_end().to_string());
            }
        }

        lines.join("\n")
    }

    /// Get last N lines
    pub async fn get_last_lines(&self, n: usize) -> Vec<String> {
        let term = self.term.lock().await;
        let grid = term.grid();
        let mut lines = Vec::new();

        let total_lines = grid.total_lines();
        let start = if total_lines > n { total_lines - n } else { 0 };

        for y in start..total_lines {
            let line = alacritty_terminal::index::Line(y as i32);
            if y < grid.total_lines() {
                let row = &grid[line];
                let text: String = row.into_iter().map(|cell| cell.c).collect();
                lines.push(text.trim_end().to_string());
            }
        }

        lines
    }

    // ========== Lifecycle ==========

    /// Start the PTY session
    pub async fn start(&mut self) -> Result<()> {
        if self.running.load(Ordering::SeqCst) {
            return Err(anyhow!("Session already started"));
        }

        info!(slot_id = %self.slot_id, cwd = %self.cwd.display(), "Starting PTY session");

        // Create PTY
        let pty_system = native_pty_system();
        let pty_pair = pty_system.openpty(PtySize {
            rows: self.rows,
            cols: self.cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        // Build command: claude --add-dir "cwd"
        let mut cmd = CommandBuilder::new("zsh");
        cmd.args([
            "-l",
            "-c",
            &format!("claude --add-dir \"{}\"", self.cwd.display()),
        ]);
        cmd.cwd(&self.cwd);

        // Set environment
        cmd.env("TERM", "xterm-256color");
        if let Ok(path) = std::env::var("PATH") {
            cmd.env("PATH", path);
        }
        if let Ok(home) = std::env::var("HOME") {
            cmd.env("HOME", home);
        }

        // Spawn child process
        let mut child = pty_pair.slave.spawn_command(cmd)?;
        let pid = child.process_id().unwrap_or(0);
        *self.pty_pid.write().await = Some(pid);
        info!(pid = pid, "PTY spawned");

        // Get writer
        let writer = pty_pair.master.take_writer()?;
        *self.pty_writer.lock().await = Some(writer);

        // Get reader
        let reader = pty_pair.master.try_clone_reader()?;

        self.running.store(true, Ordering::SeqCst);

        // Create shutdown channel
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        self.shutdown_tx = Some(shutdown_tx);

        // Spawn read task
        let term = Arc::clone(&self.term);
        let event_tx = self.event_tx.clone();
        let running = Arc::clone(&self.running);

        tokio::spawn(async move {
            Self::read_loop(reader, term, event_tx, running, shutdown_rx).await;
        });

        // Spawn state check task
        let session_state = Arc::clone(&self.state);
        let term_for_check = Arc::clone(&self.term);
        let extractor = Arc::clone(&self.extractor);
        let text_assembler = Arc::clone(&self.text_assembler);
        let current_turn = Arc::clone(&self.current_turn_id);
        let stream_seq = Arc::clone(&self.stream_seq);
        let turn_counter = Arc::clone(&self.turn_counter);
        let line_source = Arc::clone(&self.line_source_by_y);
        let assistant_active = Arc::clone(&self.assistant_block_active);
        let state_change_tx = self.state_change_tx.clone();
        let event_tx_for_check = self.event_tx.clone();
        let running_for_check = Arc::clone(&self.running);
        let pending_confirm = Arc::clone(&self.pending_tool_confirm);
        let permission_check = Arc::clone(&self.permission_check);
        let pty_writer = Arc::clone(&self.pty_writer);

        tokio::spawn(async move {
            Self::state_check_loop(
                session_state,
                term_for_check,
                extractor,
                text_assembler,
                current_turn,
                stream_seq,
                turn_counter,
                line_source,
                assistant_active,
                state_change_tx,
                event_tx_for_check,
                running_for_check,
                pending_confirm,
                permission_check,
                pty_writer,
            )
            .await;
        });

        // Wait for child exit in background
        let event_tx_for_exit = self.event_tx.clone();
        let running_for_exit = Arc::clone(&self.running);
        let state_for_exit = Arc::clone(&self.state);

        tokio::spawn(async move {
            // Wait for child to exit (blocking in thread pool)
            let exit_status = tokio::task::spawn_blocking(move || child.wait())
                .await
                .ok()
                .and_then(|r| r.ok());

            let exit_code = exit_status
                .map(|s| s.exit_code() as i32)
                .unwrap_or(-1);

            running_for_exit.store(false, Ordering::SeqCst);
            *state_for_exit.write().await = SessionState::Exited;

            let _ = event_tx_for_exit.send(SessionEvent::Exit(exit_code));
            info!(exit_code = exit_code, "PTY exited");
        });

        // Wait for idle state (Claude ready)
        self.wait_for_state(SessionState::Idle, Duration::from_secs(60))
            .await?;

        Ok(())
    }

    /// Read loop - reads from PTY and feeds to terminal
    async fn read_loop(
        reader: Box<dyn Read + Send>,
        _term: Arc<Mutex<Term<SessionEventListener>>>,
        event_tx: broadcast::Sender<SessionEvent>,
        running: Arc<AtomicBool>,
        _shutdown_rx: oneshot::Receiver<()>,
    ) {
        // Move reader into a thread that will do blocking reads
        let running_clone = Arc::clone(&running);

        tokio::task::spawn_blocking(move || {
            let mut reader = reader;
            let mut buf = [0u8; 4096];

            while running_clone.load(Ordering::SeqCst) {
                match reader.read(&mut buf) {
                    Ok(0) => break, // EOF
                    Ok(n) => {
                        let data = buf[..n].to_vec();
                        // Note: We can't easily feed to terminal from here since Term is not Send
                        // The terminal feeding should happen in the main async context
                        // For now, just emit the data event
                        let _ = event_tx.send(SessionEvent::Data(data));
                    }
                    Err(e) => {
                        error!(error = %e, "PTY read error");
                        break;
                    }
                }
            }
        });
    }

    /// State check loop - periodically checks terminal state
    #[allow(clippy::too_many_arguments)]
    async fn state_check_loop(
        state: Arc<RwLock<SessionState>>,
        term: Arc<Mutex<Term<SessionEventListener>>>,
        extractor: Arc<Mutex<IncrementalExtractor>>,
        text_assembler: Arc<Mutex<TextAssembler>>,
        current_turn_id: Arc<RwLock<Option<u64>>>,
        stream_seq: Arc<RwLock<u64>>,
        turn_counter: Arc<RwLock<u64>>,
        line_source_by_y: Arc<RwLock<HashMap<usize, ScreenTextSource>>>,
        assistant_block_active: Arc<AtomicBool>,
        state_change_tx: broadcast::Sender<(SessionState, SessionState)>,
        event_tx: broadcast::Sender<SessionEvent>,
        running: Arc<AtomicBool>,
        pending_tool_confirm: Arc<RwLock<Option<ConfirmInfo>>>,
        permission_check: Arc<RwLock<Option<Box<dyn Fn(&ConfirmInfo) -> PermissionDecision + Send + Sync>>>>,
        pty_writer: Arc<Mutex<Option<Box<dyn IoWrite + Send>>>>,
    ) {
        let mut check_interval = interval(Duration::from_millis(100));

        // Create parsers (stateless, can be reused)
        let state_parser = ClaudeCodeStateParser::new();
        let confirm_parser = ClaudeCodeConfirmParser::new();
        let status_parser = ClaudeCodeStatusParser::new();
        let tool_parser = ClaudeCodeToolOutputParser::new();
        let fingerprint_registry = default_registry();

        while running.load(Ordering::SeqCst) {
            check_interval.tick().await;

            // Extract frame delta
            let delta = {
                let term_guard = term.lock().await;
                let mut extractor_guard = extractor.lock().await;
                extractor_guard.extract(&*term_guard)
            };

            // Get screen text for state detection
            let last_lines = {
                let term_guard = term.lock().await;
                let grid = term_guard.grid();
                let mut lines = Vec::new();
                let total = grid.total_lines();
                let start = if total > 20 { total - 20 } else { 0 };
                for y in start..total {
                    let line = alacritty_terminal::index::Line(y as i32);
                    if y < total {
                        let row = &grid[line];
                        let text: String = row.into_iter().map(|cell| cell.c).collect();
                        lines.push(text.trim_end().to_string());
                    }
                }
                lines
            };

            // Create ParserContext with current state
            let current_state = *state.read().await;
            let context = ParserContext::new(last_lines.clone())
                .with_state(current_state_to_semantic(current_state));

            // Use FingerprintRegistry for quick hints
            let hints = fingerprint_registry.extract(&context).hints;

            // Detect state using semantic StateParser
            let detected_result = state_parser.detect_state(&context);
            let detected_state = detected_result.as_ref().map(|r| semantic_state_to_session_state(r.state));

            // Handle state transitions
            if let Some(new_state) = detected_state {
                // Check for trust confirmation during startup (auto-confirm)
                if let Some(ref result) = detected_result {
                    if let Some(ref meta) = result.meta {
                        if meta.needs_trust_confirm == Some(true) {
                            debug!("Auto-confirming trust dialog");
                            if let Some(writer) = pty_writer.lock().await.as_mut() {
                                let _ = writer.write_all(b"\r");
                            }
                            continue;
                        }
                    }
                }

                if new_state != current_state {
                    // Begin turn when entering processing state
                    if new_state.is_processing() && !current_state.is_processing() {
                        let mut counter = turn_counter.write().await;
                        *counter += 1;
                        *current_turn_id.write().await = Some(*counter);
                        *stream_seq.write().await = 0;
                        text_assembler.lock().await.reset();
                        line_source_by_y.write().await.clear();
                        assistant_block_active.store(false, Ordering::SeqCst);
                        debug!(turn_id = *counter, "Begin turn");
                    }

                    *state.write().await = new_state;

                    // End turn when leaving processing state
                    if current_state.is_processing() && !new_state.is_processing() {
                        if let Some(turn_id) = *current_turn_id.read().await {
                            let content = text_assembler.lock().await.finalize();
                            let _ = event_tx.send(SessionEvent::TextOutput(
                                TextOutputEvent::Complete {
                                    turn_id,
                                    content,
                                    timestamp: Utc::now().timestamp_millis(),
                                },
                            ));
                            debug!(turn_id = turn_id, "End turn");
                        }
                        *current_turn_id.write().await = None;
                        *stream_seq.write().await = 0;
                        text_assembler.lock().await.reset();
                        line_source_by_y.write().await.clear();
                        assistant_block_active.store(false, Ordering::SeqCst);
                    }

                    // Handle confirming state using semantic ConfirmParser
                    if new_state == SessionState::Confirming {
                        let semantic_confirm = confirm_parser.detect_confirm(&context);
                        let confirm_info = semantic_confirm.as_ref().map(convert_semantic_confirm_info);
                        *pending_tool_confirm.write().await = confirm_info.clone();

                        // Check permission if callback is set
                        if let Some(ref info) = confirm_info {
                            let permission = permission_check.read().await;
                            if let Some(ref check_fn) = *permission {
                                let decision = check_fn(info);
                                match decision {
                                    PermissionDecision::Allow => {
                                        // Auto-approve
                                        if let Some(writer) =
                                            pty_writer.lock().await.as_mut()
                                        {
                                            let _ = writer.write_all(b"\r");
                                        }
                                        continue;
                                    }
                                    PermissionDecision::Deny => {
                                        // Auto-deny (down, down, enter)
                                        if let Some(writer) =
                                            pty_writer.lock().await.as_mut()
                                        {
                                            let _ =
                                                writer.write_all(b"\x1b[B\x1b[B\r");
                                        }
                                        continue;
                                    }
                                    PermissionDecision::Confirm => {
                                        // Require manual confirmation
                                    }
                                }
                            }
                        }

                        let _ = event_tx.send(SessionEvent::ConfirmRequired {
                            prompt: last_lines.join("\n"),
                            info: confirm_info,
                        });
                    }

                    let _ = state_change_tx.send((new_state, current_state));
                    let _ = event_tx.send(SessionEvent::StateChange {
                        new_state,
                        prev_state: current_state,
                    });
                }
            }

            // Emit StatusUpdate event if spinner is detected
            if hints.has_spinner {
                if let Some(status) = status_parser.parse(&context) {
                    let _ = event_tx.send(SessionEvent::StatusUpdate(status));
                }
            }

            // Emit ToolOutput event if tool output is detected
            if hints.has_tool_output {
                if let Some(result) = tool_parser.parse(&context) {
                    let _ = event_tx.send(SessionEvent::ToolOutput(result.data));
                }
            }

            // Process stable ops for text streaming
            if !delta.stable_ops.is_empty() && current_state.is_processing() {
                let turn_id = *current_turn_id.read().await;
                if let Some(turn_id) = turn_id {
                    for op in &delta.stable_ops {
                        let source = classify_stable_op(op);
                        if matches!(source, ScreenTextSource::Assistant) {
                            let chunk = text_assembler.lock().await.apply(op);
                            if !chunk.is_empty() {
                                let seq = {
                                    let mut seq_guard = stream_seq.write().await;
                                    let s = *seq_guard;
                                    *seq_guard += 1;
                                    s
                                };
                                let _ =
                                    event_tx.send(SessionEvent::TextOutput(TextOutputEvent::Stream {
                                        turn_id,
                                        seq,
                                        content: chunk,
                                        timestamp: Utc::now().timestamp_millis(),
                                    }));
                            }
                        }
                    }
                }
            }
        }
    }

    /// Write data to PTY
    pub async fn write(&self, data: &str) -> Result<()> {
        if !self.running.load(Ordering::SeqCst) {
            return Err(anyhow!("Session not running"));
        }

        let mut writer_guard = self.pty_writer.lock().await;
        if let Some(ref mut writer) = *writer_guard {
            writer.write_all(data.as_bytes())?;
            writer.flush()?;
            debug!(data_len = data.len(), "Wrote to PTY");
            Ok(())
        } else {
            Err(anyhow!("PTY writer not available"))
        }
    }

    /// Send message and wait for response
    pub async fn send(&self, message: &str, timeout_ms: u64) -> Result<String> {
        let state = self.state().await;
        if state != SessionState::Idle {
            return Err(anyhow!("Cannot send message in state: {:?}", state));
        }

        // Record user message
        {
            let mut history = self.history.write().await;
            history.push(Message {
                role: MessageRole::User,
                content: message.trim().to_string(),
                timestamp: Utc::now().timestamp_millis(),
            });
        }

        // Subscribe to events before sending
        let mut rx = self.event_tx.subscribe();

        // Send message
        self.write(message).await?;
        tokio::time::sleep(Duration::from_millis(100)).await;
        self.write("\r").await?;

        // Wait for state to leave idle
        self.wait_for_state_change(SessionState::Idle, Duration::from_secs(30))
            .await?;

        // Wait for response complete
        let timeout_duration = Duration::from_millis(timeout_ms);
        let result = timeout(timeout_duration, async {
            loop {
                if let Ok(event) = rx.recv().await {
                    if let SessionEvent::TextOutput(TextOutputEvent::Complete { content, .. }) =
                        event
                    {
                        return Ok(content);
                    }
                    if let SessionEvent::Exit(code) = event {
                        return Err(anyhow!("Session exited with code: {}", code));
                    }
                }
            }
        })
        .await;

        match result {
            Ok(Ok(response)) => {
                // Record assistant message
                {
                    let mut history = self.history.write().await;
                    history.push(Message {
                        role: MessageRole::Assistant,
                        content: response.clone(),
                        timestamp: Utc::now().timestamp_millis(),
                    });
                }
                Ok(response)
            }
            Ok(Err(e)) => Err(e),
            Err(_) => Err(anyhow!("Timeout waiting for response")),
        }
    }

    /// Send confirmation response
    pub async fn confirm(&self, response: ConfirmResponse) -> Result<()> {
        let state = self.state().await;
        if state != SessionState::Confirming {
            warn!(state = ?state, "Not in confirming state");
            return Ok(());
        }

        let input = match response {
            ConfirmResponse::Yes => "\r",
            ConfirmResponse::No => "\x1b[B\x1b[B\r", // down, down, enter
            ConfirmResponse::Option(n) => {
                let downs = "\x1b[B".repeat(n.saturating_sub(1));
                let input = format!("{}\r", downs);
                self.write(&input).await?;
                return Ok(());
            }
        };

        self.write(input).await
    }

    /// Send interrupt (Ctrl+C)
    pub async fn interrupt(&self) -> Result<()> {
        self.write("\x03").await
    }

    /// Set permission check callback
    pub async fn set_permission_check<F>(&self, callback: F)
    where
        F: Fn(&ConfirmInfo) -> PermissionDecision + Send + Sync + 'static,
    {
        *self.permission_check.write().await = Some(Box::new(callback));
    }

    /// Subscribe to session events
    pub fn subscribe(&self) -> broadcast::Receiver<SessionEvent> {
        self.event_tx.subscribe()
    }

    /// Subscribe to state changes
    pub fn subscribe_state_changes(&self) -> broadcast::Receiver<(SessionState, SessionState)> {
        self.state_change_tx.subscribe()
    }

    /// Wait for specific state
    pub async fn wait_for_state(&self, target: SessionState, timeout_duration: Duration) -> Result<()> {
        let current = self.state().await;
        if current == target {
            return Ok(());
        }

        let mut rx = self.state_change_tx.subscribe();

        timeout(timeout_duration, async {
            loop {
                if let Ok((new_state, _)) = rx.recv().await {
                    if new_state == target {
                        return Ok(());
                    }
                    if matches!(new_state, SessionState::Error | SessionState::Exited) {
                        return Err(anyhow!(
                            "Session entered {:?} while waiting for {:?}",
                            new_state,
                            target
                        ));
                    }
                }
            }
        })
        .await
        .map_err(|_| anyhow!("Timeout waiting for state: {:?}", target))?
    }

    /// Wait for state to change from current
    async fn wait_for_state_change(
        &self,
        current: SessionState,
        timeout_duration: Duration,
    ) -> Result<()> {
        let state = self.state().await;
        if state != current {
            return Ok(());
        }

        let mut rx = self.state_change_tx.subscribe();

        timeout(timeout_duration, async {
            loop {
                if let Ok((new_state, _)) = rx.recv().await {
                    if new_state != current {
                        return Ok(());
                    }
                }
            }
        })
        .await
        .map_err(|_| anyhow!("Timeout waiting to leave state: {:?}", current))?
    }

    /// Close session gracefully
    pub async fn close(&mut self) -> Result<()> {
        if !self.running.load(Ordering::SeqCst) {
            return Ok(());
        }

        info!("Closing PTY session");

        // Try graceful exit
        let _ = self.write("/exit\r").await;

        // Wait for exit or timeout
        let timeout_result = timeout(Duration::from_secs(3), async {
            let mut rx = self.event_tx.subscribe();
            loop {
                if let Ok(SessionEvent::Exit(_)) = rx.recv().await {
                    break;
                }
            }
        })
        .await;

        if timeout_result.is_err() {
            // Force kill
            self.kill().await;
        }

        Ok(())
    }

    /// Force kill session
    pub async fn kill(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        *self.pty_writer.lock().await = None;
        info!("PTY session killed");
    }
}

/// Confirmation response types
pub enum ConfirmResponse {
    Yes,
    No,
    Option(usize),
}

// ========== Helper Functions ==========

/// Convert semantic State to SessionState
fn semantic_state_to_session_state(state: SemanticState) -> SessionState {
    match state {
        SemanticState::Starting => SessionState::Starting,
        SemanticState::Idle => SessionState::Idle,
        SemanticState::Thinking => SessionState::Thinking,
        SemanticState::ToolRunning => SessionState::ToolRunning,
        SemanticState::Confirming => SessionState::Confirming,
        SemanticState::Error => SessionState::Error,
    }
}

/// Convert SessionState to semantic State
fn current_state_to_semantic(state: SessionState) -> SemanticState {
    match state {
        SessionState::Starting => SemanticState::Starting,
        SessionState::Idle => SemanticState::Idle,
        SessionState::Thinking => SemanticState::Thinking,
        SessionState::Responding => SemanticState::Thinking, // No direct mapping
        SessionState::ToolRunning => SemanticState::ToolRunning,
        SessionState::Confirming => SemanticState::Confirming,
        SessionState::Error => SemanticState::Error,
        SessionState::Exited => SemanticState::Idle, // No direct mapping
    }
}

/// Convert semantic ConfirmInfo to session ConfirmInfo
fn convert_semantic_confirm_info(info: &SemanticConfirmInfo) -> ConfirmInfo {
    let options: Vec<String> = info
        .options
        .as_ref()
        .map(|opts| opts.iter().map(|o| o.label.clone()).collect())
        .unwrap_or_default();

    let tool = info.tool.as_ref().map(|t| ToolInfo {
        name: t.name.clone(),
        mcp_server: t.mcp_server.clone(),
        params: t
            .params
            .iter()
            .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
            .collect(),
    });

    ConfirmInfo {
        confirm_type: format!("{:?}", info.confirm_type).to_lowercase(),
        tool,
        options,
        selected: 0, // Default to first option selected
    }
}

/// Classify stable op source
fn classify_stable_op(op: &StableTextOp) -> ScreenTextSource {
    let text = op.text();
    let trimmed = text.trim_start();

    // Prompt line = user input
    if trimmed.starts_with('>') || trimmed.starts_with('❯') {
        return ScreenTextSource::User;
    }

    // Tool output markers
    if trimmed.starts_with('⎿') || trimmed.starts_with('│') {
        return ScreenTextSource::Tool;
    }

    // Tool call header
    if trimmed.starts_with('⏺') {
        // Check if it's a tool call (has parameters) or assistant text
        if trimmed.contains('(') && !trimmed.contains("completed") {
            return ScreenTextSource::Tool;
        }
        return ScreenTextSource::Assistant;
    }

    // UI elements
    if trimmed.contains("ctrl+")
        || trimmed.contains("Ctrl+")
        || trimmed.contains("IDE disconnected")
    {
        return ScreenTextSource::Ui;
    }

    // Box drawing = UI
    if trimmed
        .chars()
        .any(|c| matches!(c, '╭' | '╮' | '╯' | '╰' | '┌' | '┐' | '└' | '┘' | '─' | '━' | '═'))
    {
        return ScreenTextSource::Ui;
    }

    ScreenTextSource::Unknown
}
