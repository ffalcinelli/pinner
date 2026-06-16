use crate::core::dependency::{DependencyName, DependencyRef};
use serde::Serialize;
use std::path::PathBuf;

/// Represents a specific location in a file that needs to be updated.
#[derive(Debug, Clone)]
pub struct UpdateTask {
    /// Path to the file containing the dependency.
    pub path: PathBuf,
    /// Byte offset where the dependency value starts.
    pub start: usize,
    /// Byte offset where the dependency value ends.
    pub end: usize,
    /// The name of the action or dependency.
    pub action: DependencyName,
    /// The current tag or ref (if any).
    pub current_tag: Option<String>,
    /// Any existing comment following the dependency.
    pub comment: Option<String>,
    /// The YAML key used (e.g., "uses", "image", "pipe").
    pub key: String,
}

/// The result of a successful update operation.
#[derive(Debug, Serialize, Clone)]
pub struct UpdateResult {
    /// The task that was executed.
    #[serde(skip)]
    pub task: UpdateTask,
    /// The name of the updated action.
    pub action: DependencyName,
    /// The path to the modified file.
    pub path: PathBuf,
    /// The previous tag or ref.
    pub old_tag: Option<String>,
    /// The new immutable SHA or digest.
    pub new_sha: DependencyRef,
    /// The new tag (used as a comment for readability).
    pub new_tag: Option<String>,
}

#[derive(Serialize)]
pub struct JsonOutput {
    pub updates: Vec<UpdateResult>,
}
