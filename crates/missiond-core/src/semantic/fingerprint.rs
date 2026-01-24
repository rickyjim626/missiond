//! Fingerprint Registry - Centralized pattern management for terminal output parsing
//!
//! This module provides a fingerprint-based pattern matching system for detecting
//! various patterns in Claude Code CLI output.

use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::ParserContext;

// ========== Types ==========

/// Type of fingerprint pattern
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FingerprintType {
    /// Regular expression pattern
    Regex,
    /// Enumerated list of strings (match any)
    Enum,
    /// Simple string contains match
    String,
    /// Marker pattern (alias for enum, used for special markers)
    Marker,
}

/// Category of fingerprint
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FingerprintCategory {
    /// Spinner indicators (activity/loading)
    Spinner,
    /// Status bar patterns
    Statusbar,
    /// Prompt patterns (input waiting)
    Prompt,
    /// Separator lines
    Separator,
    /// Assistant response markers
    Assistant,
    /// Tool invocation patterns
    Tool,
    /// Error indicators
    Error,
    /// Confirmation dialog patterns
    Confirm,
}

/// Pattern type for fingerprints
#[derive(Debug, Clone)]
pub enum FingerprintPattern {
    /// Compiled regex pattern
    Regex(Regex),
    /// Single string pattern
    String(String),
    /// List of string patterns (for enum/marker types)
    Enum(Vec<String>),
}

/// A fingerprint definition for pattern matching
#[derive(Debug, Clone)]
pub struct Fingerprint {
    /// Unique identifier
    pub id: &'static str,
    /// Type of pattern matching
    pub fingerprint_type: FingerprintType,
    /// Category for grouping
    pub category: FingerprintCategory,
    /// Pattern to match
    pub pattern: FingerprintPattern,
    /// Confidence level (0.0 - 1.0)
    pub confidence: f64,
    /// Priority (higher = checked first within category)
    pub priority: u32,
    /// Source identifier (e.g., "claude-code-v1.0")
    pub source: &'static str,
}

/// Result of matching a single fingerprint
#[derive(Debug, Clone)]
pub struct FingerprintMatch {
    /// The matched fingerprint
    pub fingerprint_id: String,
    /// Whether the pattern matched
    pub matched: bool,
    /// Captured groups (for regex patterns)
    pub captures: Option<Vec<String>>,
    /// Line index where match was found
    pub line_index: Option<usize>,
}

/// Hints derived from fingerprint matches
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FingerprintHints {
    /// Whether a spinner was detected
    pub has_spinner: bool,
    /// Whether a prompt was detected
    pub has_prompt: bool,
    /// Whether tool output was detected
    pub has_tool_output: bool,
    /// Whether a confirmation dialog was detected
    pub has_confirm_dialog: bool,
    /// Whether an error was detected
    pub has_error: bool,
}

/// Result of fingerprint extraction
#[derive(Debug, Clone)]
pub struct FingerprintResult {
    /// All matches by fingerprint ID
    pub matches: HashMap<String, FingerprintMatch>,
    /// Matches grouped by category
    pub categories: HashMap<FingerprintCategory, Vec<FingerprintMatch>>,
    /// Derived hints for quick state detection
    pub hints: FingerprintHints,
}

// ========== Registry ==========

/// Registry for fingerprint patterns
#[derive(Debug, Default)]
pub struct FingerprintRegistry {
    /// All registered fingerprints
    fingerprints: HashMap<String, Fingerprint>,
    /// Fingerprints indexed by category
    by_category: HashMap<FingerprintCategory, Vec<String>>,
}

impl FingerprintRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self {
            fingerprints: HashMap::new(),
            by_category: HashMap::new(),
        }
    }

    /// Register a fingerprint
    pub fn register(&mut self, fp: Fingerprint) {
        let id = fp.id.to_string();
        let category = fp.category;

        // Remove from old category if exists
        if let Some(existing) = self.fingerprints.get(&id) {
            let old_category = existing.category;
            if let Some(ids) = self.by_category.get_mut(&old_category) {
                ids.retain(|i| i != &id);
            }
        }

        // Add to fingerprints map
        self.fingerprints.insert(id.clone(), fp);

        // Add to category index
        let cat_ids = self.by_category.entry(category).or_default();
        if !cat_ids.contains(&id) {
            cat_ids.push(id);
        }

        // Sort by priority (descending)
        if let Some(ids) = self.by_category.get_mut(&category) {
            ids.sort_by(|a, b| {
                let fp_a = self.fingerprints.get(a);
                let fp_b = self.fingerprints.get(b);
                match (fp_a, fp_b) {
                    (Some(a), Some(b)) => b.priority.cmp(&a.priority),
                    _ => std::cmp::Ordering::Equal,
                }
            });
        }
    }

    /// Register multiple fingerprints
    pub fn register_all(&mut self, fps: Vec<Fingerprint>) {
        for fp in fps {
            self.register(fp);
        }
    }

    /// Get a fingerprint by ID
    pub fn get(&self, id: &str) -> Option<&Fingerprint> {
        self.fingerprints.get(id)
    }

    /// Get all fingerprints in a category
    pub fn get_by_category(&self, category: FingerprintCategory) -> Vec<&Fingerprint> {
        self.by_category
            .get(&category)
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| self.fingerprints.get(id))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Extract fingerprints from parser context
    pub fn extract(&self, context: &ParserContext) -> FingerprintResult {
        let mut matches = HashMap::new();
        let mut categories: HashMap<FingerprintCategory, Vec<FingerprintMatch>> = HashMap::new();

        for (id, fp) in &self.fingerprints {
            let m = self.match_fingerprint(fp, context);

            if m.matched {
                let cat_matches = categories.entry(fp.category).or_default();
                cat_matches.push(m.clone());
            }

            matches.insert(id.clone(), m);
        }

        let hints = FingerprintHints {
            has_spinner: categories.get(&FingerprintCategory::Spinner).map_or(false, |v| !v.is_empty()),
            has_prompt: categories.get(&FingerprintCategory::Prompt).map_or(false, |v| !v.is_empty()),
            has_tool_output: categories.get(&FingerprintCategory::Tool).map_or(false, |v| !v.is_empty()),
            has_confirm_dialog: categories.get(&FingerprintCategory::Confirm).map_or(false, |v| !v.is_empty()),
            has_error: categories.get(&FingerprintCategory::Error).map_or(false, |v| !v.is_empty()),
        };

        FingerprintResult {
            matches,
            categories,
            hints,
        }
    }

    /// Match a single fingerprint against context
    fn match_fingerprint(&self, fp: &Fingerprint, context: &ParserContext) -> FingerprintMatch {
        for (i, line) in context.last_lines.iter().enumerate() {
            match &fp.pattern {
                FingerprintPattern::Regex(re) => {
                    if let Some(caps) = re.captures(line) {
                        let captures: Vec<String> = caps
                            .iter()
                            .skip(1)
                            .filter_map(|m| m.map(|m| m.as_str().to_string()))
                            .collect();
                        return FingerprintMatch {
                            fingerprint_id: fp.id.to_string(),
                            matched: true,
                            captures: Some(captures),
                            line_index: Some(i),
                        };
                    }
                }
                FingerprintPattern::String(s) => {
                    if line.contains(s) {
                        return FingerprintMatch {
                            fingerprint_id: fp.id.to_string(),
                            matched: true,
                            captures: None,
                            line_index: Some(i),
                        };
                    }
                }
                FingerprintPattern::Enum(patterns) => {
                    for p in patterns {
                        if line.contains(p) {
                            return FingerprintMatch {
                                fingerprint_id: fp.id.to_string(),
                                matched: true,
                                captures: Some(vec![p.clone()]),
                                line_index: Some(i),
                            };
                        }
                    }
                }
            }
        }

        // Also check full content for string patterns
        if let FingerprintPattern::String(s) = &fp.pattern {
            if let Some(content) = &context.full_content {
                if content.contains(s) {
                    return FingerprintMatch {
                        fingerprint_id: fp.id.to_string(),
                        matched: true,
                        captures: None,
                        line_index: None,
                    };
                }
            }
        }

        FingerprintMatch {
            fingerprint_id: fp.id.to_string(),
            matched: false,
            captures: None,
            line_index: None,
        }
    }

    /// Clear all registered fingerprints
    pub fn clear(&mut self) {
        self.fingerprints.clear();
        self.by_category.clear();
    }
}

// ========== Claude Code Fingerprints ==========

/// Pre-compiled regex patterns for Claude Code
mod patterns {
    use once_cell::sync::Lazy;
    use regex::Regex;

    pub static STATUSBAR_PATTERN: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"^([·✻✽✶✳✢])\s+(\S+…?)\s*\((?:esc|ESC)\s+to\s+interrupt(?:\s*·\s*(\w+))?\)")
            .expect("Invalid statusbar regex")
    });

    pub static PROMPT_INPUT: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"^[❯>]\s*$").expect("Invalid prompt input regex")
    });

    pub static PROMPT_WITH_TEXT: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"^[❯>]\s+.+").expect("Invalid prompt with text regex")
    });

    pub static SEPARATOR: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"^[─━═]+$").expect("Invalid separator regex")
    });

    pub static TOOL_HEADER: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"^⏺\s+(\w+)(?:\s+\(completed\s+in\s+([\d.]+)s?\))?$")
            .expect("Invalid tool header regex")
    });

    pub static TOOL_INLINE_HEADER: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"^⏺\s+(\w+)\(.+\)$").expect("Invalid tool inline header regex")
    });

    pub static TOOL_PARAM: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"^\s*│\s*(\w+):\s*(.+)$").expect("Invalid tool param regex")
    });

    pub static TOOL_OUTPUT_LINE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"^\s*│\s+(.+)$").expect("Invalid tool output line regex")
    });

    pub static TOOL_INLINE_OUTPUT: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"^\s*⎿\s+.+$").expect("Invalid tool inline output regex")
    });

    pub static CONFIRM_NUMBERED: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"^\s*(\d+)\.\s+(.+)$").expect("Invalid confirm numbered regex")
    });

    pub static CONFIRM_YES: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)^\s*1\.\s+Yes,?\s").expect("Invalid confirm yes regex")
    });

    pub static CONFIRM_NO: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)^\s*\d+\.\s+No,?\s").expect("Invalid confirm no regex")
    });

    pub static ERROR_STACK_TRACE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"^\s+at\s+.+\(.+:\d+:\d+\)$").expect("Invalid error stack trace regex")
    });

    pub static TITLE_PATTERN: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"^([⠐⠂⠈⠁⠉⠃⠋⠓⠒⠖⠦⠤✳])\s+(.+)$").expect("Invalid title pattern regex")
    });
}

/// Create the default Claude Code fingerprints
pub fn claude_code_fingerprints() -> Vec<Fingerprint> {
    vec![
        // ========== Spinners ==========
        Fingerprint {
            id: "claude-code.spinner.status",
            fingerprint_type: FingerprintType::Enum,
            category: FingerprintCategory::Spinner,
            pattern: FingerprintPattern::Enum(vec![
                "·".into(), "✻".into(), "✽".into(), "✶".into(), "✳".into(), "✢".into(),
            ]),
            confidence: 0.95,
            priority: 100,
            source: "claude-code-v1.0",
        },
        Fingerprint {
            id: "claude-code.spinner.braille",
            fingerprint_type: FingerprintType::Enum,
            category: FingerprintCategory::Spinner,
            pattern: FingerprintPattern::Enum(vec![
                "⠐".into(), "⠂".into(), "⠈".into(), "⠁".into(), "⠉".into(), "⠃".into(),
                "⠋".into(), "⠓".into(), "⠒".into(), "⠖".into(), "⠦".into(), "⠤".into(),
            ]),
            confidence: 0.95,
            priority: 100,
            source: "claude-code-v1.0",
        },

        // ========== Status Bar ==========
        Fingerprint {
            id: "claude-code.statusbar.pattern",
            fingerprint_type: FingerprintType::Regex,
            category: FingerprintCategory::Statusbar,
            pattern: FingerprintPattern::Regex(patterns::STATUSBAR_PATTERN.clone()),
            confidence: 0.95,
            priority: 95,
            source: "claude-code-v1.0",
        },
        Fingerprint {
            id: "claude-code.statusbar.running",
            fingerprint_type: FingerprintType::String,
            category: FingerprintCategory::Statusbar,
            pattern: FingerprintPattern::String("esc to interrupt".into()),
            confidence: 0.90,
            priority: 90,
            source: "claude-code-v1.0",
        },

        // ========== Prompts ==========
        Fingerprint {
            id: "claude-code.prompt.input",
            fingerprint_type: FingerprintType::Regex,
            category: FingerprintCategory::Prompt,
            pattern: FingerprintPattern::Regex(patterns::PROMPT_INPUT.clone()),
            confidence: 0.90,
            priority: 90,
            source: "claude-code-v1.0",
        },
        Fingerprint {
            id: "claude-code.prompt.with-text",
            fingerprint_type: FingerprintType::Regex,
            category: FingerprintCategory::Prompt,
            pattern: FingerprintPattern::Regex(patterns::PROMPT_WITH_TEXT.clone()),
            confidence: 0.85,
            priority: 85,
            source: "claude-code-v1.0",
        },

        // ========== Markers ==========
        Fingerprint {
            id: "claude-code.marker.response",
            fingerprint_type: FingerprintType::String,
            category: FingerprintCategory::Assistant,
            pattern: FingerprintPattern::String("⏺".into()),
            confidence: 0.95,
            priority: 90,
            source: "claude-code-v1.0",
        },
        Fingerprint {
            id: "claude-code.marker.separator",
            fingerprint_type: FingerprintType::Regex,
            category: FingerprintCategory::Separator,
            pattern: FingerprintPattern::Regex(patterns::SEPARATOR.clone()),
            confidence: 0.90,
            priority: 80,
            source: "claude-code-v1.0",
        },

        // ========== Tool Output ==========
        Fingerprint {
            id: "claude-code.tool.header",
            fingerprint_type: FingerprintType::Regex,
            category: FingerprintCategory::Tool,
            pattern: FingerprintPattern::Regex(patterns::TOOL_HEADER.clone()),
            confidence: 0.95,
            priority: 92,
            source: "claude-code-v1.0",
        },
        Fingerprint {
            id: "claude-code.tool.inline-header",
            fingerprint_type: FingerprintType::Regex,
            category: FingerprintCategory::Tool,
            pattern: FingerprintPattern::Regex(patterns::TOOL_INLINE_HEADER.clone()),
            confidence: 0.90,
            priority: 92,
            source: "claude-code-v1.0",
        },
        Fingerprint {
            id: "claude-code.tool.param",
            fingerprint_type: FingerprintType::Regex,
            category: FingerprintCategory::Tool,
            pattern: FingerprintPattern::Regex(patterns::TOOL_PARAM.clone()),
            confidence: 0.90,
            priority: 90,
            source: "claude-code-v1.0",
        },
        Fingerprint {
            id: "claude-code.tool.output-line",
            fingerprint_type: FingerprintType::Regex,
            category: FingerprintCategory::Tool,
            pattern: FingerprintPattern::Regex(patterns::TOOL_OUTPUT_LINE.clone()),
            confidence: 0.85,
            priority: 85,
            source: "claude-code-v1.0",
        },
        Fingerprint {
            id: "claude-code.tool.inline-output-line",
            fingerprint_type: FingerprintType::Regex,
            category: FingerprintCategory::Tool,
            pattern: FingerprintPattern::Regex(patterns::TOOL_INLINE_OUTPUT.clone()),
            confidence: 0.85,
            priority: 85,
            source: "claude-code-v1.0",
        },
        Fingerprint {
            id: "claude-code.tool.known-names",
            fingerprint_type: FingerprintType::Enum,
            category: FingerprintCategory::Tool,
            pattern: FingerprintPattern::Enum(vec![
                "Bash".into(), "Read".into(), "Edit".into(), "Write".into(),
                "Glob".into(), "Grep".into(), "WebFetch".into(), "WebSearch".into(),
                "Task".into(), "LSP".into(), "NotebookEdit".into(),
                "TodoRead".into(), "TodoWrite".into(),
            ]),
            confidence: 0.95,
            priority: 92,
            source: "claude-code-v1.0",
        },

        // ========== Confirm Dialog ==========
        Fingerprint {
            id: "claude-code.confirm.numbered-option",
            fingerprint_type: FingerprintType::Regex,
            category: FingerprintCategory::Confirm,
            pattern: FingerprintPattern::Regex(patterns::CONFIRM_NUMBERED.clone()),
            confidence: 0.85,
            priority: 85,
            source: "claude-code-v1.0",
        },
        Fingerprint {
            id: "claude-code.confirm.yes-option",
            fingerprint_type: FingerprintType::Regex,
            category: FingerprintCategory::Confirm,
            pattern: FingerprintPattern::Regex(patterns::CONFIRM_YES.clone()),
            confidence: 0.90,
            priority: 88,
            source: "claude-code-v1.0",
        },
        Fingerprint {
            id: "claude-code.confirm.no-option",
            fingerprint_type: FingerprintType::Regex,
            category: FingerprintCategory::Confirm,
            pattern: FingerprintPattern::Regex(patterns::CONFIRM_NO.clone()),
            confidence: 0.90,
            priority: 88,
            source: "claude-code-v1.0",
        },

        // ========== Error Markers ==========
        Fingerprint {
            id: "claude-code.error.keywords",
            fingerprint_type: FingerprintType::Enum,
            category: FingerprintCategory::Error,
            pattern: FingerprintPattern::Enum(vec![
                "Error:".into(), "error:".into(), "ERROR:".into(),
                "✖".into(), "ENOENT".into(), "EPERM".into(), "EACCES".into(),
                "failed".into(), "Failed".into(),
            ]),
            confidence: 0.85,
            priority: 80,
            source: "claude-code-v1.0",
        },
        Fingerprint {
            id: "claude-code.error.stack-trace",
            fingerprint_type: FingerprintType::Regex,
            category: FingerprintCategory::Error,
            pattern: FingerprintPattern::Regex(patterns::ERROR_STACK_TRACE.clone()),
            confidence: 0.90,
            priority: 82,
            source: "claude-code-v1.0",
        },

        // ========== Title Patterns ==========
        Fingerprint {
            id: "claude-code.title.pattern",
            fingerprint_type: FingerprintType::Regex,
            category: FingerprintCategory::Statusbar,
            pattern: FingerprintPattern::Regex(patterns::TITLE_PATTERN.clone()),
            confidence: 0.90,
            priority: 85,
            source: "claude-code-v1.0",
        },
    ]
}

/// Pre-built Claude Code fingerprints as a lazy static
pub static CLAUDE_CODE_FINGERPRINTS: Lazy<Vec<Fingerprint>> = Lazy::new(claude_code_fingerprints);

/// Create a new registry pre-loaded with Claude Code fingerprints
pub fn default_registry() -> FingerprintRegistry {
    let mut registry = FingerprintRegistry::new();
    registry.register_all(claude_code_fingerprints());
    registry
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_register() {
        let mut registry = FingerprintRegistry::new();
        registry.register_all(claude_code_fingerprints());

        assert!(registry.get("claude-code.spinner.status").is_some());
        assert!(registry.get("claude-code.prompt.input").is_some());
        assert!(registry.get("nonexistent").is_none());
    }

    #[test]
    fn test_get_by_category() {
        let registry = default_registry();

        let spinners = registry.get_by_category(FingerprintCategory::Spinner);
        assert!(!spinners.is_empty());

        let prompts = registry.get_by_category(FingerprintCategory::Prompt);
        assert!(!prompts.is_empty());
    }

    #[test]
    fn test_extract_spinner() {
        let registry = default_registry();
        let context = ParserContext::new(vec!["·".to_string()]);
        let result = registry.extract(&context);

        assert!(result.hints.has_spinner);
    }

    #[test]
    fn test_extract_prompt() {
        let registry = default_registry();
        let context = ParserContext::new(vec!["❯ ".to_string()]);
        let result = registry.extract(&context);

        assert!(result.hints.has_prompt);
    }

    #[test]
    fn test_extract_statusbar() {
        let registry = default_registry();
        let context = ParserContext::new(vec![
            "✻ Reading file... (esc to interrupt)".to_string()
        ]);
        let result = registry.extract(&context);

        // Should match the string pattern
        let matches = result.matches.get("claude-code.statusbar.running");
        assert!(matches.is_some());
        assert!(matches.unwrap().matched);
    }

    #[test]
    fn test_extract_error() {
        let registry = default_registry();
        let context = ParserContext::new(vec!["Error: file not found".to_string()]);
        let result = registry.extract(&context);

        assert!(result.hints.has_error);
    }

    #[test]
    fn test_extract_tool() {
        let registry = default_registry();
        let context = ParserContext::new(vec!["⏺ Read".to_string()]);
        let result = registry.extract(&context);

        assert!(result.hints.has_tool_output);
    }

    #[test]
    fn test_fingerprint_hints_default() {
        let hints = FingerprintHints::default();
        assert!(!hints.has_spinner);
        assert!(!hints.has_prompt);
        assert!(!hints.has_tool_output);
        assert!(!hints.has_confirm_dialog);
        assert!(!hints.has_error);
    }
}
