//! Semantic terminal parsing module
//!
//! This module provides parsers for detecting terminal states and parsing
//! confirmation dialogs from Claude Code CLI output.

mod confirm;
pub mod fingerprint;
mod state;
mod status;
mod title;
mod tool;
mod types;

pub use confirm::ClaudeCodeConfirmParser;
pub use fingerprint::{
    claude_code_fingerprints, default_registry, Fingerprint, FingerprintCategory,
    FingerprintHints, FingerprintMatch, FingerprintPattern, FingerprintRegistry,
    FingerprintResult, FingerprintType, CLAUDE_CODE_FINGERPRINTS,
};
pub use state::ClaudeCodeStateParser;
pub use status::{ClaudeCodeStatusParser, SPINNER_CHARS};
pub use title::{ClaudeCodeTitleParser, ALL_SPINNERS, BRAILLE_SPINNERS, OTHER_SPINNERS};
pub use tool::{ClaudeCodeToolOutputParser, KNOWN_TOOLS};
pub use types::*;
