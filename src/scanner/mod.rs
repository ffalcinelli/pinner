//! The scanner module is responsible for traversing the file system and parsing
//! CI/CD workflow files to identify dependencies that need updating.
//!
//! It uses `tree-sitter-yaml` for robust AST-based parsing, which allows it to
//! find dependencies even in complex YAML structures while preserving comments
//! and formatting information.

pub mod parser;
pub mod walker;

pub use parser::find_tasks;
pub use walker::Scanner;
