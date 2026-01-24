//! Type definitions for semantic terminal parsing
//!
//! This module defines the core types used by state and confirm parsers.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Terminal state detected by state parsers
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// Starting up, may need trust confirmation
    Starting,
    /// Idle, waiting for user input (prompt visible)
    Idle,
    /// Thinking/processing (esc to interrupt visible)
    Thinking,
    /// Running a tool
    ToolRunning,
    /// Waiting for user confirmation
    Confirming,
    /// Error state
    Error,
}

impl std::fmt::Display for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            State::Starting => write!(f, "starting"),
            State::Idle => write!(f, "idle"),
            State::Thinking => write!(f, "thinking"),
            State::ToolRunning => write!(f, "tool_running"),
            State::Confirming => write!(f, "confirming"),
            State::Error => write!(f, "error"),
        }
    }
}

/// Parser metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParserMeta {
    /// Parser name
    pub name: String,
    /// Parser description
    pub description: String,
    /// Parser priority (higher = checked first)
    pub priority: u32,
    /// Parser version
    pub version: String,
}

/// Context provided to parsers
#[derive(Debug, Clone)]
pub struct ParserContext {
    /// Last N lines of terminal output
    pub last_lines: Vec<String>,
    /// Current detected state (if any)
    pub current_state: Option<State>,
    /// Full terminal content (optional, for complex parsing)
    pub full_content: Option<String>,
}

impl ParserContext {
    /// Create a new parser context from lines
    pub fn new(last_lines: Vec<String>) -> Self {
        Self {
            last_lines,
            current_state: None,
            full_content: None,
        }
    }

    /// Create context with current state
    pub fn with_state(mut self, state: State) -> Self {
        self.current_state = Some(state);
        self
    }

    /// Create context with full content
    pub fn with_full_content(mut self, content: String) -> Self {
        self.full_content = Some(content);
        self
    }

    /// Get joined text from last lines
    pub fn text(&self) -> String {
        self.last_lines.join("\n")
    }
}

/// State detection metadata
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StateMeta {
    /// Whether trust confirmation is needed (during startup)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub needs_trust_confirm: Option<bool>,
    /// Confirm type if in confirming state
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confirm_type: Option<ConfirmType>,
}

/// Result of state detection
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateDetectionResult {
    /// Detected state
    pub state: State,
    /// Confidence level (0.0 - 1.0)
    pub confidence: f64,
    /// Additional metadata
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meta: Option<StateMeta>,
}

impl StateDetectionResult {
    /// Create a new state detection result
    pub fn new(state: State, confidence: f64) -> Self {
        Self {
            state,
            confidence,
            meta: None,
        }
    }

    /// Add metadata to the result
    pub fn with_meta(mut self, meta: StateMeta) -> Self {
        self.meta = Some(meta);
        self
    }
}

/// Trait for state parsers
pub trait StateParser {
    /// Get parser metadata
    fn meta(&self) -> &ParserMeta;

    /// Detect state from parser context
    fn detect_state(&self, context: &ParserContext) -> Option<StateDetectionResult>;
}

// ============ Confirm Types ============

/// Type of confirmation dialog
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfirmType {
    /// Options-style confirm (1. Yes, 2. ..., 3. No)
    Options,
    /// Simple Y/n confirm
    YesNo,
}

/// A single confirm option
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfirmOption {
    /// Option key (number or character)
    pub key: ConfirmKey,
    /// Option label
    pub label: String,
    /// Whether this is the default option
    #[serde(default)]
    pub is_default: bool,
}

/// Key type for confirm options
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ConfirmKey {
    /// Numeric key (1, 2, 3, ...)
    Number(u32),
    /// Character key (y, n, ...)
    Char(String),
}

impl std::fmt::Display for ConfirmKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfirmKey::Number(n) => write!(f, "{}", n),
            ConfirmKey::Char(c) => write!(f, "{}", c),
        }
    }
}

/// Information about a tool being confirmed
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolInfo {
    /// Tool name
    pub name: String,
    /// MCP server name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mcp_server: Option<String>,
    /// Tool parameters
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub params: HashMap<String, String>,
}

/// Information about a confirmation dialog
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfirmInfo {
    /// Type of confirmation
    #[serde(rename = "type")]
    pub confirm_type: ConfirmType,
    /// Prompt text
    pub prompt: String,
    /// Available options
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<ConfirmOption>>,
    /// Tool information (if confirming tool usage)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<ToolInfo>,
    /// Raw prompt text
    pub raw_prompt: String,
}

/// Action to take in response to confirmation
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfirmAction {
    /// Confirm (Yes)
    Confirm,
    /// Deny (No)
    Deny,
    /// Select a specific option
    Select,
    /// Input custom text
    Input,
}

/// Response to a confirmation dialog
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfirmResponse {
    /// Action to take
    pub action: ConfirmAction,
    /// Option number (for Select action)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub option: Option<u32>,
    /// Custom value (for Input action)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

impl ConfirmResponse {
    /// Create a confirm response
    pub fn confirm() -> Self {
        Self {
            action: ConfirmAction::Confirm,
            option: None,
            value: None,
        }
    }

    /// Create a deny response
    pub fn deny() -> Self {
        Self {
            action: ConfirmAction::Deny,
            option: None,
            value: None,
        }
    }

    /// Create a select response
    pub fn select(option: u32) -> Self {
        Self {
            action: ConfirmAction::Select,
            option: Some(option),
            value: None,
        }
    }

    /// Create an input response
    pub fn input(value: impl Into<String>) -> Self {
        Self {
            action: ConfirmAction::Input,
            option: None,
            value: Some(value.into()),
        }
    }
}

/// Trait for confirm parsers
pub trait ConfirmParser {
    /// Get parser metadata
    fn meta(&self) -> &ParserMeta;

    /// Detect and parse confirmation dialog
    fn detect_confirm(&self, context: &ParserContext) -> Option<ConfirmInfo>;

    /// Format a response for the terminal
    fn format_response(&self, info: &ConfirmInfo, response: &ConfirmResponse) -> String;
}

// ============ Title Types ============

/// Information parsed from Claude Code terminal title
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaudeCodeTitle {
    /// Current task name (if any)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_name: Option<String>,
    /// Spinner character state
    pub spinner_state: String,
    /// Whether Claude is currently processing
    pub is_processing: bool,
}

/// Context for title parsing
#[derive(Debug, Clone)]
pub struct TitleParserContext {
    /// The terminal title to parse
    pub terminal_title: String,
}

impl TitleParserContext {
    /// Create a new title parser context
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            terminal_title: title.into(),
        }
    }
}

/// Result of title parsing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TitleParseResult {
    /// Output type
    pub output_type: String,
    /// Raw title text
    pub raw: String,
    /// Parsed data
    pub data: ClaudeCodeTitle,
    /// Confidence level (0.0 - 1.0)
    pub confidence: f64,
}

/// Trait for title parsers
pub trait TitleParser {
    /// Get parser metadata
    fn meta(&self) -> &ParserMeta;

    /// Check if this parser can handle the given title
    fn can_parse(&self, context: &TitleParserContext) -> bool;

    /// Parse the terminal title
    fn parse(&self, context: &TitleParserContext) -> Option<TitleParseResult>;
}

// ============ Tool Output Types ============

/// Status of a tool execution
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    /// Tool is currently running
    Running,
    /// Tool has completed
    Completed,
}

impl std::fmt::Display for ToolStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ToolStatus::Running => write!(f, "running"),
            ToolStatus::Completed => write!(f, "completed"),
        }
    }
}

/// Parsed tool output from Claude Code
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaudeCodeToolOutput {
    /// Tool name (e.g., "Bash", "Read", "Edit")
    pub tool_name: String,
    /// Tool parameters
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub params: HashMap<String, serde_json::Value>,
    /// Tool output content
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    /// Duration in milliseconds (if completed)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<f64>,
    /// Tool execution status
    pub status: ToolStatus,
}

/// Result of tool output parsing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolOutputResult {
    /// Output type (always "claude-tool")
    #[serde(rename = "type")]
    pub output_type: String,
    /// Raw text that was parsed
    pub raw: String,
    /// Parsed tool data
    pub data: ClaudeCodeToolOutput,
    /// Parser confidence (0.0 - 1.0)
    pub confidence: f64,
}

/// Trait for tool output parsers
pub trait ToolOutputParser {
    /// Get parser metadata
    fn meta(&self) -> &ParserMeta;

    /// Check if the context can be parsed as tool output
    fn can_parse(&self, context: &ParserContext) -> bool;

    /// Parse tool output from context
    fn parse(&self, context: &ParserContext) -> Option<ToolOutputResult>;
}

// ============ Status Types ============

/// Phase of Claude Code status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusPhase {
    /// Thinking/processing
    Thinking,
    /// Running a tool
    ToolRunning,
    /// Unknown phase
    Unknown,
}

impl std::fmt::Display for StatusPhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StatusPhase::Thinking => write!(f, "thinking"),
            StatusPhase::ToolRunning => write!(f, "tool_running"),
            StatusPhase::Unknown => write!(f, "unknown"),
        }
    }
}

/// Claude Code status bar information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaudeCodeStatus {
    /// Spinner character
    pub spinner: String,
    /// Status text (e.g., "Precipitating...")
    pub status_text: String,
    /// Current phase
    pub phase: StatusPhase,
    /// Whether the operation can be interrupted
    pub interruptible: bool,
}

/// Trait for status parsers
pub trait StatusParser {
    /// Get parser metadata
    fn meta(&self) -> &ParserMeta;

    /// Check if the parser can parse the given context
    fn can_parse(&self, context: &ParserContext) -> bool;

    /// Parse status from context
    fn parse(&self, context: &ParserContext) -> Option<ClaudeCodeStatus>;
}
