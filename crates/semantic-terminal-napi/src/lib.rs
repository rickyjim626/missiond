//! NAPI bindings for semantic-terminal
//!
//! Provides Node.js bindings for Claude Code terminal output parsing.

use napi_derive::napi;
use std::collections::HashMap;

use missiond_core::semantic::{
    self, ClaudeCodeConfirmParser, ClaudeCodeStateParser, ClaudeCodeStatusParser,
    ClaudeCodeTitleParser, ClaudeCodeToolOutputParser, ConfirmParser as ConfirmParserTrait,
    FingerprintCategory as RustFingerprintCategory, FingerprintRegistry,
    StateParser as StateParserTrait, StatusParser as StatusParserTrait,
    TitleParser as TitleParserTrait, ToolOutputParser as ToolOutputParserTrait,
};

// ============ State Types ============

/// Terminal state detected by state parsers
#[napi(string_enum)]
pub enum State {
    Starting,
    Idle,
    Thinking,
    ToolRunning,
    Confirming,
    Error,
}

impl From<semantic::State> for State {
    fn from(s: semantic::State) -> Self {
        match s {
            semantic::State::Starting => State::Starting,
            semantic::State::Idle => State::Idle,
            semantic::State::Thinking => State::Thinking,
            semantic::State::ToolRunning => State::ToolRunning,
            semantic::State::Confirming => State::Confirming,
            semantic::State::Error => State::Error,
        }
    }
}

/// Type of confirmation dialog
#[napi(string_enum)]
pub enum ConfirmType {
    Options,
    YesNo,
}

impl From<semantic::ConfirmType> for ConfirmType {
    fn from(t: semantic::ConfirmType) -> Self {
        match t {
            semantic::ConfirmType::Options => ConfirmType::Options,
            semantic::ConfirmType::YesNo => ConfirmType::YesNo,
        }
    }
}

/// Status phase
#[napi(string_enum)]
pub enum StatusPhase {
    Thinking,
    ToolRunning,
    Unknown,
}

impl From<semantic::StatusPhase> for StatusPhase {
    fn from(p: semantic::StatusPhase) -> Self {
        match p {
            semantic::StatusPhase::Thinking => StatusPhase::Thinking,
            semantic::StatusPhase::ToolRunning => StatusPhase::ToolRunning,
            semantic::StatusPhase::Unknown => StatusPhase::Unknown,
        }
    }
}

/// Tool execution status
#[napi(string_enum)]
pub enum ToolStatus {
    Running,
    Completed,
}

impl From<semantic::ToolStatus> for ToolStatus {
    fn from(s: semantic::ToolStatus) -> Self {
        match s {
            semantic::ToolStatus::Running => ToolStatus::Running,
            semantic::ToolStatus::Completed => ToolStatus::Completed,
        }
    }
}

/// Fingerprint category
#[napi(string_enum)]
pub enum FingerprintCategory {
    Spinner,
    Statusbar,
    Prompt,
    Separator,
    Assistant,
    Tool,
    Error,
    Confirm,
}

impl From<RustFingerprintCategory> for FingerprintCategory {
    fn from(c: RustFingerprintCategory) -> Self {
        match c {
            RustFingerprintCategory::Spinner => FingerprintCategory::Spinner,
            RustFingerprintCategory::Statusbar => FingerprintCategory::Statusbar,
            RustFingerprintCategory::Prompt => FingerprintCategory::Prompt,
            RustFingerprintCategory::Separator => FingerprintCategory::Separator,
            RustFingerprintCategory::Assistant => FingerprintCategory::Assistant,
            RustFingerprintCategory::Tool => FingerprintCategory::Tool,
            RustFingerprintCategory::Error => FingerprintCategory::Error,
            RustFingerprintCategory::Confirm => FingerprintCategory::Confirm,
        }
    }
}

// ============ Result Types ============

/// State detection result
#[napi(object)]
pub struct StateResult {
    pub state: State,
    pub confidence: f64,
    pub needs_trust_confirm: Option<bool>,
    pub confirm_type: Option<ConfirmType>,
}

/// Confirm option
#[napi(object)]
pub struct ConfirmOption {
    pub key: String,
    pub label: String,
    pub is_default: bool,
}

/// Tool info in confirmation
#[napi(object)]
pub struct ToolInfo {
    pub name: String,
    pub mcp_server: Option<String>,
    pub params: HashMap<String, String>,
}

/// Confirmation dialog info
#[napi(object)]
pub struct ConfirmInfo {
    pub confirm_type: ConfirmType,
    pub prompt: String,
    pub options: Option<Vec<ConfirmOption>>,
    pub tool: Option<ToolInfo>,
    pub raw_prompt: String,
}

/// Status bar info
#[napi(object)]
pub struct StatusInfo {
    pub spinner: String,
    pub status_text: String,
    pub phase: StatusPhase,
    pub interruptible: bool,
}

/// Title parse result
#[napi(object)]
pub struct TitleInfo {
    pub task_name: Option<String>,
    pub spinner_state: String,
    pub is_processing: bool,
}

/// Tool output result
#[napi(object)]
pub struct ToolOutput {
    pub tool_name: String,
    pub params: HashMap<String, serde_json::Value>,
    pub output: Option<String>,
    pub duration_ms: Option<f64>,
    pub status: ToolStatus,
}

/// Fingerprint match result
#[napi(object)]
pub struct FingerprintMatch {
    pub fingerprint_id: String,
    pub matched: bool,
    pub captures: Option<Vec<String>>,
    pub line_index: Option<u32>,
}

/// Fingerprint hints
#[napi(object)]
pub struct FingerprintHints {
    pub has_spinner: bool,
    pub has_prompt: bool,
    pub has_tool_output: bool,
    pub has_confirm_dialog: bool,
    pub has_error: bool,
}

/// Fingerprint extraction result
#[napi(object)]
pub struct FingerprintResult {
    pub hints: FingerprintHints,
    pub matches: HashMap<String, FingerprintMatch>,
}

// ============ Parser Classes ============

/// Claude Code state parser
#[napi]
pub struct StateParser {
    inner: ClaudeCodeStateParser,
}

#[napi]
impl StateParser {
    #[napi(constructor)]
    pub fn new() -> Self {
        Self {
            inner: ClaudeCodeStateParser::new(),
        }
    }

    /// Detect state from terminal lines
    #[napi]
    pub fn detect(&self, lines: Vec<String>, current_state: Option<State>) -> Option<StateResult> {
        let mut context = semantic::ParserContext::new(lines);

        if let Some(state) = current_state {
            let rust_state = match state {
                State::Starting => semantic::State::Starting,
                State::Idle => semantic::State::Idle,
                State::Thinking => semantic::State::Thinking,
                State::ToolRunning => semantic::State::ToolRunning,
                State::Confirming => semantic::State::Confirming,
                State::Error => semantic::State::Error,
            };
            context = context.with_state(rust_state);
        }

        self.inner.detect_state(&context).map(|result| {
            let (needs_trust_confirm, confirm_type) = result
                .meta
                .map(|m| {
                    (
                        m.needs_trust_confirm,
                        m.confirm_type.map(ConfirmType::from),
                    )
                })
                .unwrap_or((None, None));

            StateResult {
                state: result.state.into(),
                confidence: result.confidence,
                needs_trust_confirm,
                confirm_type,
            }
        })
    }
}

/// Claude Code confirmation parser
#[napi]
pub struct ConfirmParser {
    inner: ClaudeCodeConfirmParser,
}

#[napi]
impl ConfirmParser {
    #[napi(constructor)]
    pub fn new() -> Self {
        Self {
            inner: ClaudeCodeConfirmParser::new(),
        }
    }

    /// Detect and parse confirmation dialog
    #[napi]
    pub fn detect(&self, lines: Vec<String>) -> Option<ConfirmInfo> {
        let context = semantic::ParserContext::new(lines);

        self.inner.detect_confirm(&context).map(|info| {
            let options = info.options.map(|opts| {
                opts.into_iter()
                    .map(|o| ConfirmOption {
                        key: o.key.to_string(),
                        label: o.label,
                        is_default: o.is_default,
                    })
                    .collect()
            });

            let tool = info.tool.map(|t| ToolInfo {
                name: t.name,
                mcp_server: t.mcp_server,
                params: t.params,
            });

            ConfirmInfo {
                confirm_type: info.confirm_type.into(),
                prompt: info.prompt,
                options,
                tool,
                raw_prompt: info.raw_prompt,
            }
        })
    }

    /// Format a response for the terminal
    #[napi]
    pub fn format_confirm(&self) -> String {
        "\r".to_string()
    }

    /// Format a deny response for the terminal
    #[napi]
    pub fn format_deny(&self, confirm_type: ConfirmType) -> String {
        match confirm_type {
            ConfirmType::Options => "\x1b[B\x1b[B\r".to_string(),
            ConfirmType::YesNo => "n\r".to_string(),
        }
    }

    /// Format a select response for the terminal
    #[napi]
    pub fn format_select(&self, option: u32) -> String {
        if option > 1 {
            let down_keys = "\x1b[B".repeat((option - 1) as usize);
            format!("{}\r", down_keys)
        } else {
            "\r".to_string()
        }
    }
}

/// Claude Code status parser
#[napi]
pub struct StatusParser {
    inner: ClaudeCodeStatusParser,
}

#[napi]
impl StatusParser {
    #[napi(constructor)]
    pub fn new() -> Self {
        Self {
            inner: ClaudeCodeStatusParser::new(),
        }
    }

    /// Check if context can be parsed as status
    #[napi]
    pub fn can_parse(&self, lines: Vec<String>) -> bool {
        let context = semantic::ParserContext::new(lines);
        self.inner.can_parse(&context)
    }

    /// Parse status bar information
    #[napi]
    pub fn parse(&self, lines: Vec<String>) -> Option<StatusInfo> {
        let context = semantic::ParserContext::new(lines);

        self.inner.parse(&context).map(|status| StatusInfo {
            spinner: status.spinner,
            status_text: status.status_text,
            phase: status.phase.into(),
            interruptible: status.interruptible,
        })
    }
}

/// Claude Code title parser
#[napi]
pub struct TitleParser {
    inner: ClaudeCodeTitleParser,
}

#[napi]
impl TitleParser {
    #[napi(constructor)]
    pub fn new() -> Self {
        Self {
            inner: ClaudeCodeTitleParser::new(),
        }
    }

    /// Check if title can be parsed
    #[napi]
    pub fn can_parse(&self, title: String) -> bool {
        let context = semantic::TitleParserContext::new(title);
        self.inner.can_parse(&context)
    }

    /// Parse terminal title
    #[napi]
    pub fn parse(&self, title: String) -> Option<TitleInfo> {
        let context = semantic::TitleParserContext::new(title);

        self.inner.parse(&context).map(|result| TitleInfo {
            task_name: result.data.task_name,
            spinner_state: result.data.spinner_state,
            is_processing: result.data.is_processing,
        })
    }
}

/// Claude Code tool output parser
#[napi]
pub struct ToolOutputParser {
    inner: ClaudeCodeToolOutputParser,
}

#[napi]
impl ToolOutputParser {
    #[napi(constructor)]
    pub fn new() -> Self {
        Self {
            inner: ClaudeCodeToolOutputParser::new(),
        }
    }

    /// Check if context can be parsed as tool output
    #[napi]
    pub fn can_parse(&self, lines: Vec<String>) -> bool {
        let context = semantic::ParserContext::new(lines);
        self.inner.can_parse(&context)
    }

    /// Parse tool output
    #[napi]
    pub fn parse(&self, lines: Vec<String>) -> Option<ToolOutput> {
        let context = semantic::ParserContext::new(lines);

        self.inner.parse(&context).map(|result| ToolOutput {
            tool_name: result.data.tool_name,
            params: result.data.params,
            output: result.data.output,
            duration_ms: result.data.duration_ms,
            status: result.data.status.into(),
        })
    }
}

/// Fingerprint registry for pattern matching
#[napi]
pub struct Registry {
    inner: FingerprintRegistry,
}

#[napi]
impl Registry {
    /// Create a new registry with default Claude Code fingerprints
    #[napi(constructor)]
    pub fn new() -> Self {
        Self {
            inner: semantic::default_registry(),
        }
    }

    /// Extract fingerprints from terminal lines
    #[napi]
    pub fn extract(&self, lines: Vec<String>) -> FingerprintResult {
        let context = semantic::ParserContext::new(lines);
        let result = self.inner.extract(&context);

        let matches: HashMap<String, FingerprintMatch> = result
            .matches
            .into_iter()
            .map(|(id, m)| {
                (
                    id,
                    FingerprintMatch {
                        fingerprint_id: m.fingerprint_id,
                        matched: m.matched,
                        captures: m.captures,
                        line_index: m.line_index.map(|i| i as u32),
                    },
                )
            })
            .collect();

        FingerprintResult {
            hints: FingerprintHints {
                has_spinner: result.hints.has_spinner,
                has_prompt: result.hints.has_prompt,
                has_tool_output: result.hints.has_tool_output,
                has_confirm_dialog: result.hints.has_confirm_dialog,
                has_error: result.hints.has_error,
            },
            matches,
        }
    }

    /// Quick check for spinner
    #[napi]
    pub fn has_spinner(&self, lines: Vec<String>) -> bool {
        let context = semantic::ParserContext::new(lines);
        self.inner.extract(&context).hints.has_spinner
    }

    /// Quick check for prompt
    #[napi]
    pub fn has_prompt(&self, lines: Vec<String>) -> bool {
        let context = semantic::ParserContext::new(lines);
        self.inner.extract(&context).hints.has_prompt
    }

    /// Quick check for error
    #[napi]
    pub fn has_error(&self, lines: Vec<String>) -> bool {
        let context = semantic::ParserContext::new(lines);
        self.inner.extract(&context).hints.has_error
    }
}

// ============ Convenience Functions ============

/// Detect state from terminal lines (convenience function)
#[napi]
pub fn detect_state(lines: Vec<String>) -> Option<StateResult> {
    let parser = StateParser::new();
    parser.detect(lines, None)
}

/// Detect confirmation dialog (convenience function)
#[napi]
pub fn detect_confirm(lines: Vec<String>) -> Option<ConfirmInfo> {
    let parser = ConfirmParser::new();
    parser.detect(lines)
}

/// Parse status bar (convenience function)
#[napi]
pub fn parse_status(lines: Vec<String>) -> Option<StatusInfo> {
    let parser = StatusParser::new();
    parser.parse(lines)
}

/// Parse tool output (convenience function)
#[napi]
pub fn parse_tool_output(lines: Vec<String>) -> Option<ToolOutput> {
    let parser = ToolOutputParser::new();
    parser.parse(lines)
}

/// Extract fingerprints (convenience function)
#[napi]
pub fn extract_fingerprints(lines: Vec<String>) -> FingerprintResult {
    let registry = Registry::new();
    registry.extract(lines)
}

/// Known tool names
#[napi]
pub fn known_tools() -> Vec<String> {
    semantic::KNOWN_TOOLS.iter().map(|s| s.to_string()).collect()
}

/// Spinner characters used by Claude Code
#[napi]
pub fn spinner_chars() -> Vec<String> {
    semantic::SPINNER_CHARS.iter().map(|c| c.to_string()).collect()
}
