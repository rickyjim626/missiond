//! Claude Code status parser
//!
//! Parses status bar information (spinner + status text) from Claude Code CLI output.

use once_cell::sync::Lazy;
use regex::Regex;

use super::types::{ClaudeCodeStatus, ParserContext, ParserMeta, StatusParser, StatusPhase};

/// Spinner characters used by Claude Code
pub const SPINNER_CHARS: &[char] = &['·', '✻', '✽', '✶', '✳', '✢'];

/// Status text pattern: spinner + text + (esc to interrupt)
/// Example: "· Precipitating… (esc to interrupt · thinking)"
/// Example: "✻ Schlepping… (esc to interrupt)"
static STATUS_PATTERN: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^([·✻✽✶✳✢])\s+(\S+…?)\s*\((?:esc|ESC)\s+to\s+interrupt(?:\s*·\s*(\w+))?\)")
        .unwrap()
});

/// Claude Code status parser
///
/// Parses status bar information:
/// - Spinner character
/// - Status text (e.g., "Precipitating...")
/// - Phase hint (thinking, tool, etc.)
/// - Interruptible state
pub struct ClaudeCodeStatusParser {
    meta: ParserMeta,
}

impl Default for ClaudeCodeStatusParser {
    fn default() -> Self {
        Self::new()
    }
}

impl ClaudeCodeStatusParser {
    /// Create a new Claude Code status parser
    pub fn new() -> Self {
        Self {
            meta: ParserMeta {
                name: "claude-code-status".to_string(),
                description: "Parses Claude Code status bar (spinner + status text)".to_string(),
                priority: 95,
                version: "1.0.0".to_string(),
            },
        }
    }

    /// Determine phase from hint or status text
    fn determine_phase(&self, spinner: &str, status_text: &str, phase_hint: Option<&str>) -> StatusPhase {
        // Check phase hint first
        if let Some(hint) = phase_hint {
            if hint == "thinking" {
                return StatusPhase::Thinking;
            }
            if hint == "tool" {
                return StatusPhase::ToolRunning;
            }
        }

        // Check status text for tool indicators
        let status_lower = status_text.to_lowercase();
        if status_lower.contains("tool") {
            return StatusPhase::ToolRunning;
        }

        // Default to thinking if spinner is active
        if SPINNER_CHARS.iter().any(|c| spinner.contains(*c)) {
            return StatusPhase::Thinking;
        }

        StatusPhase::Unknown
    }
}

impl StatusParser for ClaudeCodeStatusParser {
    fn meta(&self) -> &ParserMeta {
        &self.meta
    }

    fn can_parse(&self, context: &ParserContext) -> bool {
        context
            .last_lines
            .iter()
            .any(|line| STATUS_PATTERN.is_match(line.trim()))
    }

    fn parse(&self, context: &ParserContext) -> Option<ClaudeCodeStatus> {
        for line in &context.last_lines {
            let trimmed = line.trim();
            if let Some(caps) = STATUS_PATTERN.captures(trimmed) {
                let spinner = caps.get(1)?.as_str().to_string();
                let status_text = caps.get(2)?.as_str().to_string();
                let phase_hint = caps.get(3).map(|m| m.as_str());

                let phase = self.determine_phase(&spinner, &status_text, phase_hint);

                return Some(ClaudeCodeStatus {
                    spinner,
                    status_text,
                    phase,
                    interruptible: true, // Always true when "esc to interrupt" is shown
                });
            }
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_context(lines: &[&str]) -> ParserContext {
        ParserContext::new(lines.iter().map(|s| s.to_string()).collect())
    }

    #[test]
    fn test_spinner_chars() {
        assert_eq!(SPINNER_CHARS.len(), 6);
        assert!(SPINNER_CHARS.contains(&'·'));
        assert!(SPINNER_CHARS.contains(&'✻'));
        assert!(SPINNER_CHARS.contains(&'✽'));
        assert!(SPINNER_CHARS.contains(&'✶'));
        assert!(SPINNER_CHARS.contains(&'✳'));
        assert!(SPINNER_CHARS.contains(&'✢'));
    }

    #[test]
    fn test_can_parse_with_status() {
        let parser = ClaudeCodeStatusParser::new();

        let context = make_context(&["· Precipitating… (esc to interrupt · thinking)"]);
        assert!(parser.can_parse(&context));

        let context = make_context(&["✻ Schlepping… (esc to interrupt)"]);
        assert!(parser.can_parse(&context));

        let context = make_context(&["✽ Processing… (ESC to interrupt)"]);
        assert!(parser.can_parse(&context));
    }

    #[test]
    fn test_can_parse_without_status() {
        let parser = ClaudeCodeStatusParser::new();

        let context = make_context(&["random text", "no status here"]);
        assert!(!parser.can_parse(&context));

        let context = make_context(&["❯ "]);
        assert!(!parser.can_parse(&context));
    }

    #[test]
    fn test_parse_with_thinking_hint() {
        let parser = ClaudeCodeStatusParser::new();

        let context = make_context(&["· Precipitating… (esc to interrupt · thinking)"]);
        let result = parser.parse(&context);

        assert!(result.is_some());
        let status = result.unwrap();
        assert_eq!(status.spinner, "·");
        assert_eq!(status.status_text, "Precipitating…");
        assert_eq!(status.phase, StatusPhase::Thinking);
        assert!(status.interruptible);
    }

    #[test]
    fn test_parse_without_hint() {
        let parser = ClaudeCodeStatusParser::new();

        let context = make_context(&["✻ Schlepping… (esc to interrupt)"]);
        let result = parser.parse(&context);

        assert!(result.is_some());
        let status = result.unwrap();
        assert_eq!(status.spinner, "✻");
        assert_eq!(status.status_text, "Schlepping…");
        assert_eq!(status.phase, StatusPhase::Thinking); // Default to thinking with spinner
        assert!(status.interruptible);
    }

    #[test]
    fn test_parse_with_tool_hint() {
        let parser = ClaudeCodeStatusParser::new();

        let context = make_context(&["✶ Running… (esc to interrupt · tool)"]);
        let result = parser.parse(&context);

        assert!(result.is_some());
        let status = result.unwrap();
        assert_eq!(status.spinner, "✶");
        assert_eq!(status.status_text, "Running…");
        assert_eq!(status.phase, StatusPhase::ToolRunning);
        assert!(status.interruptible);
    }

    #[test]
    fn test_parse_tool_in_status_text() {
        let parser = ClaudeCodeStatusParser::new();

        let context = make_context(&["✳ Tool… (esc to interrupt)"]);
        let result = parser.parse(&context);

        assert!(result.is_some());
        let status = result.unwrap();
        assert_eq!(status.phase, StatusPhase::ToolRunning);
    }

    #[test]
    fn test_parse_case_insensitive_esc() {
        let parser = ClaudeCodeStatusParser::new();

        // Lowercase esc
        let context = make_context(&["· Working… (esc to interrupt)"]);
        assert!(parser.can_parse(&context));
        assert!(parser.parse(&context).is_some());

        // Uppercase ESC
        let context = make_context(&["· Working… (ESC to interrupt)"]);
        assert!(parser.can_parse(&context));
        assert!(parser.parse(&context).is_some());
    }

    #[test]
    fn test_parse_all_spinners() {
        let parser = ClaudeCodeStatusParser::new();

        for spinner in SPINNER_CHARS {
            let line = format!("{} Status… (esc to interrupt)", spinner);
            let context = make_context(&[&line]);
            let result = parser.parse(&context);
            assert!(result.is_some(), "Failed for spinner: {}", spinner);
            assert_eq!(result.unwrap().spinner, spinner.to_string());
        }
    }

    #[test]
    fn test_parse_with_leading_whitespace() {
        let parser = ClaudeCodeStatusParser::new();

        let context = make_context(&["  · Working… (esc to interrupt · thinking)"]);
        assert!(parser.can_parse(&context));

        let result = parser.parse(&context);
        assert!(result.is_some());
        assert_eq!(result.unwrap().spinner, "·");
    }

    #[test]
    fn test_parse_returns_none_for_invalid() {
        let parser = ClaudeCodeStatusParser::new();

        let context = make_context(&["random text"]);
        assert!(parser.parse(&context).is_none());

        let context = make_context(&["· Missing parentheses"]);
        assert!(parser.parse(&context).is_none());

        let context = make_context(&["X Invalid… (esc to interrupt)"]);
        assert!(parser.parse(&context).is_none());
    }

    #[test]
    fn test_parser_meta() {
        let parser = ClaudeCodeStatusParser::new();
        let meta = parser.meta();

        assert_eq!(meta.name, "claude-code-status");
        assert_eq!(meta.priority, 95);
        assert_eq!(meta.version, "1.0.0");
    }

    #[test]
    fn test_multiple_lines_finds_status() {
        let parser = ClaudeCodeStatusParser::new();

        let context = make_context(&[
            "Some output text",
            "More output",
            "· Processing… (esc to interrupt · thinking)",
            "Other stuff",
        ]);

        assert!(parser.can_parse(&context));
        let result = parser.parse(&context);
        assert!(result.is_some());
        assert_eq!(result.unwrap().status_text, "Processing…");
    }
}
