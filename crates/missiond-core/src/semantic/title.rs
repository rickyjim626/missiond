//! Claude Code title parser
//!
//! Parses terminal title information (from OSC sequence or context.terminalTitle)

use once_cell::sync::Lazy;
use regex::Regex;

use super::types::{
    ClaudeCodeTitle, ParserMeta, TitleParseResult, TitleParser, TitleParserContext,
};

/// Braille spinner characters (used in terminal title)
pub const BRAILLE_SPINNERS: &[char] = &[
    '⠐', '⠂', '⠈', '⠁', '⠉', '⠃', '⠋', '⠓', '⠒', '⠖', '⠦', '⠤',
];

/// Other spinner indicators
pub const OTHER_SPINNERS: &[char] = &['✳', '✻', '✽', '✶', '✢', '·'];

/// All spinner characters combined
pub static ALL_SPINNERS: Lazy<Vec<char>> = Lazy::new(|| {
    let mut spinners = BRAILLE_SPINNERS.to_vec();
    spinners.extend(OTHER_SPINNERS);
    spinners
});

/// Title pattern: spinner + task name
/// Example: "⠐ Initial Greeting"
/// Example: "✳ Claude Code"
static TITLE_PATTERN: Lazy<Regex> = Lazy::new(|| {
    let spinner_chars: String = ALL_SPINNERS.iter().collect();
    Regex::new(&format!(r"^([{spinner_chars}])\s*(.*)$")).unwrap()
});

/// Claude Code title parser
///
/// Parses terminal title to extract:
/// - Task name
/// - Spinner state
/// - Processing status
pub struct ClaudeCodeTitleParser {
    meta: ParserMeta,
}

impl Default for ClaudeCodeTitleParser {
    fn default() -> Self {
        Self::new()
    }
}

impl ClaudeCodeTitleParser {
    /// Create a new Claude Code title parser
    pub fn new() -> Self {
        Self {
            meta: ParserMeta {
                name: "claude-code-title".to_string(),
                description: "Parses Claude Code terminal title".to_string(),
                priority: 85,
                version: "1.0.0".to_string(),
            },
        }
    }

    /// Check if a spinner character indicates processing
    fn is_processing_spinner(spinner: char) -> bool {
        // Braille spinners indicate active processing
        if BRAILLE_SPINNERS.contains(&spinner) {
            return true;
        }
        // Other spinners except '✳' indicate processing
        OTHER_SPINNERS.contains(&spinner) && spinner != '✳'
    }

    /// Create output with the given parameters
    fn create_output(
        &self,
        output_type: &str,
        raw: &str,
        data: ClaudeCodeTitle,
        confidence: f64,
    ) -> TitleParseResult {
        TitleParseResult {
            output_type: output_type.to_string(),
            raw: raw.to_string(),
            data,
            confidence,
        }
    }
}

impl TitleParser for ClaudeCodeTitleParser {
    fn meta(&self) -> &ParserMeta {
        &self.meta
    }

    fn can_parse(&self, context: &TitleParserContext) -> bool {
        !context.terminal_title.is_empty()
    }

    fn parse(&self, context: &TitleParserContext) -> Option<TitleParseResult> {
        let title = &context.terminal_title;
        if title.is_empty() {
            return None;
        }

        if let Some(captures) = TITLE_PATTERN.captures(title) {
            let spinner_state = captures.get(1).map(|m| m.as_str()).unwrap_or("");
            let task_name = captures.get(2).map(|m| m.as_str().trim()).unwrap_or("");

            // Get first char for processing check
            let spinner_char = spinner_state.chars().next().unwrap_or(' ');
            let is_processing = Self::is_processing_spinner(spinner_char);

            let data = ClaudeCodeTitle {
                task_name: if task_name.is_empty() {
                    None
                } else {
                    Some(task_name.to_string())
                },
                spinner_state: spinner_state.to_string(),
                is_processing,
            };

            return Some(self.create_output("claude-title", title, data, 0.95));
        }

        // No spinner, just a static title
        let data = ClaudeCodeTitle {
            task_name: if title.trim().is_empty() {
                None
            } else {
                Some(title.trim().to_string())
            },
            spinner_state: String::new(),
            is_processing: false,
        };

        Some(self.create_output("claude-title", title, data, 0.7))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_context(title: &str) -> TitleParserContext {
        TitleParserContext::new(title)
    }

    #[test]
    fn test_parse_braille_spinner_title() {
        let parser = ClaudeCodeTitleParser::new();

        // Test with braille spinner
        let context = make_context("⠐ Initial Greeting");
        let result = parser.parse(&context);
        assert!(result.is_some());
        let result = result.unwrap();
        assert_eq!(result.data.task_name, Some("Initial Greeting".to_string()));
        assert_eq!(result.data.spinner_state, "⠐");
        assert!(result.data.is_processing);
        assert!(result.confidence >= 0.9);
    }

    #[test]
    fn test_parse_different_braille_spinners() {
        let parser = ClaudeCodeTitleParser::new();

        for &spinner in BRAILLE_SPINNERS {
            let title = format!("{} Running task", spinner);
            let context = make_context(&title);
            let result = parser.parse(&context).unwrap();
            assert!(
                result.data.is_processing,
                "Braille spinner {} should indicate processing",
                spinner
            );
            assert_eq!(result.data.spinner_state, spinner.to_string());
        }
    }

    #[test]
    fn test_parse_static_spinner_title() {
        let parser = ClaudeCodeTitleParser::new();

        // Test with ✳ spinner (static/idle)
        let context = make_context("✳ Claude Code");
        let result = parser.parse(&context);
        assert!(result.is_some());
        let result = result.unwrap();
        assert_eq!(result.data.task_name, Some("Claude Code".to_string()));
        assert_eq!(result.data.spinner_state, "✳");
        assert!(!result.data.is_processing); // ✳ is not processing
    }

    #[test]
    fn test_parse_other_spinners() {
        let parser = ClaudeCodeTitleParser::new();

        // Test other spinners (should indicate processing except ✳)
        for &spinner in OTHER_SPINNERS {
            let title = format!("{} Some task", spinner);
            let context = make_context(&title);
            let result = parser.parse(&context).unwrap();

            if spinner == '✳' {
                assert!(
                    !result.data.is_processing,
                    "✳ should NOT indicate processing"
                );
            } else {
                assert!(
                    result.data.is_processing,
                    "Spinner {} should indicate processing",
                    spinner
                );
            }
        }
    }

    #[test]
    fn test_parse_static_title() {
        let parser = ClaudeCodeTitleParser::new();

        // Test with plain title (no spinner)
        let context = make_context("Claude Code");
        let result = parser.parse(&context);
        assert!(result.is_some());
        let result = result.unwrap();
        assert_eq!(result.data.task_name, Some("Claude Code".to_string()));
        assert_eq!(result.data.spinner_state, "");
        assert!(!result.data.is_processing);
        assert!(result.confidence < 0.9); // Lower confidence for static titles
    }

    #[test]
    fn test_parse_empty_title() {
        let parser = ClaudeCodeTitleParser::new();

        let context = make_context("");
        let result = parser.parse(&context);
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_spinner_only() {
        let parser = ClaudeCodeTitleParser::new();

        // Test with spinner but no task name
        let context = make_context("⠐ ");
        let result = parser.parse(&context);
        assert!(result.is_some());
        let result = result.unwrap();
        assert_eq!(result.data.task_name, None);
        assert_eq!(result.data.spinner_state, "⠐");
        assert!(result.data.is_processing);
    }

    #[test]
    fn test_can_parse() {
        let parser = ClaudeCodeTitleParser::new();

        assert!(parser.can_parse(&make_context("Some title")));
        assert!(!parser.can_parse(&make_context("")));
    }

    #[test]
    fn test_parser_meta() {
        let parser = ClaudeCodeTitleParser::new();
        let meta = parser.meta();

        assert_eq!(meta.name, "claude-code-title");
        assert_eq!(meta.priority, 85);
    }

    #[test]
    fn test_all_spinners_constant() {
        // Verify ALL_SPINNERS contains all expected characters
        assert_eq!(BRAILLE_SPINNERS.len(), 12);
        assert_eq!(OTHER_SPINNERS.len(), 6);
        assert_eq!(ALL_SPINNERS.len(), 18);
    }
}
