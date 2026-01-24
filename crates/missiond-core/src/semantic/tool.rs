//! Claude Code Tool Output Parser
//!
//! Parses tool call boxes (tool name, parameters, output) from Claude Code CLI output.

use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashMap;

use super::types::{
    ClaudeCodeToolOutput, ParserContext, ParserMeta, ToolOutputParser, ToolOutputResult,
    ToolStatus,
};

/// Tool header patterns:
/// - Box style: "⏺ Bash" or "⏺ Bash (completed in 0.5s)"
static TOOL_HEADER_BOX_PATTERN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^⏺\s+(\w+)(?:\s+\(completed\s+in\s+([\d.]+)s?\))?$").unwrap());

/// Tool header inline style: "⏺ Bash(git status)" or "⏺ Search(pattern: \"*.ts\")"
static TOOL_HEADER_INLINE_PATTERN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^⏺\s+(\w+)\((.*)\)$").unwrap());

/// Tool parameter line pattern: │ key: value
/// Example: "  │ command: \"git status\""
/// Example: "  │ file_path: \"/path/to/file\""
static PARAM_LINE_PATTERN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^\s*│\s*(\w+):\s*(.+)$").unwrap());

/// Inline tool output lines often start with ⎿
static INLINE_OUTPUT_LINE_PATTERN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^\s*⎿\s*(.+)$").unwrap());

/// Known tool names
pub const KNOWN_TOOLS: &[&str] = &[
    "Bash",
    "Read",
    "Edit",
    "Write",
    "Glob",
    "Grep",
    "WebFetch",
    "WebSearch",
    "Task",
    "LSP",
    "NotebookEdit",
    "Search",
    "TodoRead",
    "TodoWrite",
];

/// Tool style detected from header
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolStyle {
    /// Box style with │ delimited parameters
    Box,
    /// Inline style with parenthesized arguments
    Inline,
}

/// Claude Code tool output parser
///
/// Parses tool call boxes from Claude Code CLI output, extracting:
/// - Tool name
/// - Parameters
/// - Output content
/// - Duration (if completed)
/// - Status (running/completed)
pub struct ClaudeCodeToolOutputParser {
    meta: ParserMeta,
}

impl Default for ClaudeCodeToolOutputParser {
    fn default() -> Self {
        Self::new()
    }
}

impl ClaudeCodeToolOutputParser {
    /// Create a new Claude Code tool output parser
    pub fn new() -> Self {
        Self {
            meta: ParserMeta {
                name: "claude-code-tool".to_string(),
                description: "Parses Claude Code tool call outputs".to_string(),
                priority: 92,
                version: "1.0.0".to_string(),
            },
        }
    }

    /// Parse inline arguments like "git status" or "pattern: \"*.ts\", path: \"/src\""
    fn parse_inline_args(
        &self,
        tool_name: &str,
        args: &str,
    ) -> HashMap<String, serde_json::Value> {
        let trimmed = args.trim();
        if trimmed.is_empty() {
            return HashMap::new();
        }

        // Bash(...) is usually a raw command string
        if tool_name == "Bash" {
            let mut params = HashMap::new();
            params.insert("command".to_string(), serde_json::Value::String(trimmed.to_string()));
            return params;
        }

        // If it looks like key: value pairs, parse loosely
        if trimmed.contains(':') {
            let parts = self.split_args(trimmed);
            let mut out = HashMap::new();

            for part in parts {
                if let Some(idx) = part.find(':') {
                    let key = part[..idx].trim();
                    let value_raw = part[idx + 1..].trim();

                    if key.is_empty() {
                        continue;
                    }

                    // Try to parse as JSON, fallback to string
                    let value = if let Ok(v) = serde_json::from_str::<serde_json::Value>(value_raw)
                    {
                        v
                    } else {
                        // Remove surrounding quotes if present
                        let cleaned = if value_raw.starts_with('"') && value_raw.ends_with('"') {
                            &value_raw[1..value_raw.len() - 1]
                        } else {
                            value_raw
                        };
                        serde_json::Value::String(cleaned.to_string())
                    };

                    out.insert(key.to_string(), value);
                }
            }

            if !out.is_empty() {
                return out;
            }
        }

        // Fallback: store as 'args'
        let mut params = HashMap::new();
        params.insert("args".to_string(), serde_json::Value::String(trimmed.to_string()));
        params
    }

    /// Split arguments by comma, respecting quoted strings
    fn split_args(&self, args: &str) -> Vec<String> {
        let mut parts = Vec::new();
        let mut current = String::new();
        let mut in_string: Option<char> = None;

        for (i, ch) in args.chars().enumerate() {
            if let Some(quote_char) = in_string {
                if ch == quote_char && !args[..i].ends_with('\\') {
                    in_string = None;
                }
                current.push(ch);
                continue;
            }

            if ch == '"' || ch == '\'' {
                in_string = Some(ch);
                current.push(ch);
                continue;
            }

            if ch == ',' {
                if !current.is_empty() {
                    parts.push(current.clone());
                }
                current.clear();
                continue;
            }

            current.push(ch);
        }

        if !current.is_empty() {
            parts.push(current);
        }

        parts
    }

    /// Check if a tool name is known
    fn is_known_tool(&self, name: &str) -> bool {
        KNOWN_TOOLS.contains(&name)
    }
}

impl ToolOutputParser for ClaudeCodeToolOutputParser {
    fn meta(&self) -> &ParserMeta {
        &self.meta
    }

    fn can_parse(&self, context: &ParserContext) -> bool {
        // Check if any line matches tool header pattern
        context.last_lines.iter().any(|line| {
            let trimmed = line.trim();
            TOOL_HEADER_BOX_PATTERN.is_match(trimmed)
                || TOOL_HEADER_INLINE_PATTERN.is_match(trimmed)
                || INLINE_OUTPUT_LINE_PATTERN.is_match(trimmed)
        })
    }

    fn parse(&self, context: &ParserContext) -> Option<ToolOutputResult> {
        let lines = &context.last_lines;
        let mut tool_name: Option<String> = None;
        let mut duration_ms: Option<f64> = None;
        let mut params: HashMap<String, serde_json::Value> = HashMap::new();
        let mut output_lines: Vec<String> = Vec::new();
        let mut in_tool_block = false;
        let mut tool_style: Option<ToolStyle> = None;
        let mut raw_lines: Vec<String> = Vec::new();

        for line in lines {
            let trimmed = line.trim();

            // Check for box-style tool header
            if let Some(caps) = TOOL_HEADER_BOX_PATTERN.captures(trimmed) {
                tool_name = Some(caps.get(1).unwrap().as_str().to_string());
                if let Some(duration_match) = caps.get(2) {
                    // Convert seconds to milliseconds
                    if let Ok(secs) = duration_match.as_str().parse::<f64>() {
                        duration_ms = Some(secs * 1000.0);
                    }
                }
                in_tool_block = true;
                tool_style = Some(ToolStyle::Box);
                raw_lines.push(line.clone());
                continue;
            }

            // Check for inline-style tool header
            if let Some(caps) = TOOL_HEADER_INLINE_PATTERN.captures(trimmed) {
                let name = caps.get(1).unwrap().as_str();
                tool_name = Some(name.to_string());
                let arg_string = caps.get(2).map(|m| m.as_str()).unwrap_or("");
                params = self.parse_inline_args(name, arg_string);
                in_tool_block = true;
                tool_style = Some(ToolStyle::Inline);
                raw_lines.push(line.clone());
                continue;
            }

            if in_tool_block {
                match tool_style {
                    Some(ToolStyle::Box) => {
                        // Check for parameter line
                        if let Some(caps) = PARAM_LINE_PATTERN.captures(trimmed) {
                            let key = caps.get(1).unwrap().as_str();
                            let value_raw = caps.get(2).unwrap().as_str();

                            // Try to parse JSON value, fallback to string
                            let value =
                                if let Ok(v) = serde_json::from_str::<serde_json::Value>(value_raw)
                                {
                                    v
                                } else {
                                    // Remove surrounding quotes if present
                                    let cleaned =
                                        if value_raw.starts_with('"') && value_raw.ends_with('"') {
                                            &value_raw[1..value_raw.len() - 1]
                                        } else {
                                            value_raw
                                        };
                                    serde_json::Value::String(cleaned.to_string())
                                };

                            params.insert(key.to_string(), value);
                            raw_lines.push(line.clone());
                            continue;
                        }

                        // Check for output content (lines starting with │ but not key: value)
                        if trimmed.starts_with('│') {
                            let content = trimmed[3..].trim(); // Skip "│ "
                            if !content.is_empty() && !PARAM_LINE_PATTERN.is_match(trimmed) {
                                output_lines.push(content.to_string());
                                raw_lines.push(line.clone());
                            }
                            continue;
                        }

                        // End of tool block
                        if !trimmed.is_empty() && !trimmed.starts_with('│') {
                            break;
                        }
                    }
                    Some(ToolStyle::Inline) => {
                        // Check for inline output line
                        if let Some(caps) = INLINE_OUTPUT_LINE_PATTERN.captures(trimmed) {
                            let content = caps.get(1).map(|m| m.as_str().trim()).unwrap_or("");
                            if !content.is_empty() {
                                output_lines.push(content.to_string());
                            }
                            raw_lines.push(line.clone());
                            continue;
                        }

                        // Continuation lines (indented), keep as plain text output
                        if line.starts_with("  ")
                            && !trimmed.starts_with('⏺')
                            && !trimmed.starts_with('❯')
                            && !trimmed.starts_with('>')
                        {
                            output_lines.push(trimmed.to_string());
                            raw_lines.push(line.clone());
                            continue;
                        }

                        // End of tool block
                        if !trimmed.is_empty() && !INLINE_OUTPUT_LINE_PATTERN.is_match(trimmed) {
                            break;
                        }
                    }
                    None => {}
                }
            }
        }

        let tool_name = tool_name?;

        // Determine status
        let status = if duration_ms.is_some() {
            ToolStatus::Completed
        } else {
            ToolStatus::Running
        };

        let data = ClaudeCodeToolOutput {
            tool_name: tool_name.clone(),
            params,
            output: if output_lines.is_empty() {
                None
            } else {
                Some(output_lines.join("\n"))
            },
            duration_ms,
            status,
        };

        let raw = raw_lines.join("\n");
        let confidence = if self.is_known_tool(&tool_name) {
            0.95
        } else {
            0.8
        };

        Some(ToolOutputResult {
            output_type: "claude-tool".to_string(),
            raw,
            data,
            confidence,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_context(lines: &[&str]) -> ParserContext {
        ParserContext::new(lines.iter().map(|s| s.to_string()).collect())
    }

    #[test]
    fn test_can_parse_box_header() {
        let parser = ClaudeCodeToolOutputParser::new();

        let context = make_context(&["⏺ Bash", "  │ command: \"git status\""]);
        assert!(parser.can_parse(&context));
    }

    #[test]
    fn test_can_parse_inline_header() {
        let parser = ClaudeCodeToolOutputParser::new();

        let context = make_context(&["⏺ Bash(git status)"]);
        assert!(parser.can_parse(&context));
    }

    #[test]
    fn test_can_parse_completed_header() {
        let parser = ClaudeCodeToolOutputParser::new();

        let context = make_context(&["⏺ Bash (completed in 0.5s)"]);
        assert!(parser.can_parse(&context));
    }

    #[test]
    fn test_cannot_parse_random_text() {
        let parser = ClaudeCodeToolOutputParser::new();

        let context = make_context(&["random text", "nothing special"]);
        assert!(!parser.can_parse(&context));
    }

    #[test]
    fn test_parse_box_style_basic() {
        let parser = ClaudeCodeToolOutputParser::new();

        let context = make_context(&["⏺ Bash", "  │ command: \"git status\""]);
        let result = parser.parse(&context);

        assert!(result.is_some());
        let result = result.unwrap();
        assert_eq!(result.data.tool_name, "Bash");
        assert_eq!(result.data.status, ToolStatus::Running);
        assert!(result.data.duration_ms.is_none());
        assert_eq!(
            result.data.params.get("command"),
            Some(&serde_json::Value::String("git status".to_string()))
        );
        assert_eq!(result.confidence, 0.95);
    }

    #[test]
    fn test_parse_box_style_completed() {
        let parser = ClaudeCodeToolOutputParser::new();

        let context = make_context(&["⏺ Bash (completed in 0.5s)", "  │ command: \"ls -la\""]);
        let result = parser.parse(&context);

        assert!(result.is_some());
        let result = result.unwrap();
        assert_eq!(result.data.tool_name, "Bash");
        assert_eq!(result.data.status, ToolStatus::Completed);
        assert_eq!(result.data.duration_ms, Some(500.0));
    }

    #[test]
    fn test_parse_inline_style_bash() {
        let parser = ClaudeCodeToolOutputParser::new();

        let context = make_context(&["⏺ Bash(git status)"]);
        let result = parser.parse(&context);

        assert!(result.is_some());
        let result = result.unwrap();
        assert_eq!(result.data.tool_name, "Bash");
        assert_eq!(
            result.data.params.get("command"),
            Some(&serde_json::Value::String("git status".to_string()))
        );
    }

    #[test]
    fn test_parse_inline_style_with_key_value() {
        let parser = ClaudeCodeToolOutputParser::new();

        let context = make_context(&["⏺ Search(pattern: \"*.ts\", path: \"/src\")"]);
        let result = parser.parse(&context);

        assert!(result.is_some());
        let result = result.unwrap();
        assert_eq!(result.data.tool_name, "Search");
        assert_eq!(
            result.data.params.get("pattern"),
            Some(&serde_json::Value::String("*.ts".to_string()))
        );
        assert_eq!(
            result.data.params.get("path"),
            Some(&serde_json::Value::String("/src".to_string()))
        );
    }

    #[test]
    fn test_parse_with_output() {
        let parser = ClaudeCodeToolOutputParser::new();

        let context = make_context(&[
            "⏺ Bash(git status)",
            "  ⎿ On branch main",
            "  ⎿ nothing to commit",
        ]);
        let result = parser.parse(&context);

        assert!(result.is_some());
        let result = result.unwrap();
        assert_eq!(result.data.tool_name, "Bash");
        assert!(result.data.output.is_some());
        let output = result.data.output.unwrap();
        assert!(output.contains("On branch main"));
        assert!(output.contains("nothing to commit"));
    }

    #[test]
    fn test_parse_unknown_tool() {
        let parser = ClaudeCodeToolOutputParser::new();

        let context = make_context(&["⏺ UnknownTool(some args)"]);
        let result = parser.parse(&context);

        assert!(result.is_some());
        let result = result.unwrap();
        assert_eq!(result.data.tool_name, "UnknownTool");
        assert_eq!(result.confidence, 0.8); // Lower confidence for unknown tools
    }

    #[test]
    fn test_parse_read_tool() {
        let parser = ClaudeCodeToolOutputParser::new();

        let context = make_context(&[
            "⏺ Read",
            "  │ file_path: \"/path/to/file.rs\"",
            "  │ limit: 100",
        ]);
        let result = parser.parse(&context);

        assert!(result.is_some());
        let result = result.unwrap();
        assert_eq!(result.data.tool_name, "Read");
        assert_eq!(
            result.data.params.get("file_path"),
            Some(&serde_json::Value::String("/path/to/file.rs".to_string()))
        );
        // Note: "100" parsed as JSON becomes a number, but our parser stores as string
        // when JSON parsing fails
    }

    #[test]
    fn test_parse_edit_tool() {
        let parser = ClaudeCodeToolOutputParser::new();

        let context = make_context(&[
            "⏺ Edit",
            "  │ file_path: \"/src/main.rs\"",
            "  │ old_string: \"fn main\"",
            "  │ new_string: \"fn main_v2\"",
        ]);
        let result = parser.parse(&context);

        assert!(result.is_some());
        let result = result.unwrap();
        assert_eq!(result.data.tool_name, "Edit");
        assert!(result.data.params.contains_key("file_path"));
        assert!(result.data.params.contains_key("old_string"));
        assert!(result.data.params.contains_key("new_string"));
    }

    #[test]
    fn test_parse_inline_args_splitting() {
        let parser = ClaudeCodeToolOutputParser::new();

        // Test with commas inside quoted strings
        let args = parser.parse_inline_args("Search", r#"pattern: "a,b,c", path: "/src""#);
        assert_eq!(
            args.get("pattern"),
            Some(&serde_json::Value::String("a,b,c".to_string()))
        );
        assert_eq!(
            args.get("path"),
            Some(&serde_json::Value::String("/src".to_string()))
        );
    }

    #[test]
    fn test_known_tools() {
        let parser = ClaudeCodeToolOutputParser::new();

        assert!(parser.is_known_tool("Bash"));
        assert!(parser.is_known_tool("Read"));
        assert!(parser.is_known_tool("Edit"));
        assert!(parser.is_known_tool("Write"));
        assert!(parser.is_known_tool("Glob"));
        assert!(parser.is_known_tool("Grep"));
        assert!(parser.is_known_tool("WebFetch"));
        assert!(parser.is_known_tool("WebSearch"));
        assert!(!parser.is_known_tool("CustomTool"));
    }

    #[test]
    fn test_output_type() {
        let parser = ClaudeCodeToolOutputParser::new();

        let context = make_context(&["⏺ Bash(ls)"]);
        let result = parser.parse(&context).unwrap();

        assert_eq!(result.output_type, "claude-tool");
    }

    #[test]
    fn test_parser_meta() {
        let parser = ClaudeCodeToolOutputParser::new();
        let meta = parser.meta();

        assert_eq!(meta.name, "claude-code-tool");
        assert_eq!(meta.priority, 92);
        assert_eq!(meta.version, "1.0.0");
    }
}
