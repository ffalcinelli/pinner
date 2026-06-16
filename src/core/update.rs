use crate::core::dependency::{CiProvider, DependencyName, DependencyRef};
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
    /// Line number where the dependency is located (1-based).
    pub line: usize,
    /// Column number where the dependency is located (1-based).
    pub column: usize,
    /// The name of the action or dependency.
    pub action: DependencyName,
    /// The current tag or ref (if any).
    pub current_tag: Option<String>,
    /// Any existing comment following the dependency.
    pub comment: Option<String>,
    /// The YAML key used (e.g., "uses", "image", "pipe").
    pub key: String,
    /// The CI provider for this task.
    pub provider: CiProvider,
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

#[derive(Debug, Serialize, Clone)]
pub struct UnpinnedDependency {
    pub path: PathBuf,
    pub action: DependencyName,
    pub tag: Option<String>,
    pub line: usize,
    pub column: usize,
}

#[derive(Debug, Serialize, Clone, Default)]
pub struct VerificationResult {
    pub unpinned: Vec<UnpinnedDependency>,
}

impl VerificationResult {
    pub fn is_success(&self) -> bool {
        self.unpinned.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_verification_result_success() {
        let res = VerificationResult::default();
        assert!(res.is_success());
    }

    #[test]
    fn test_verification_result_failure() {
        let mut res = VerificationResult::default();
        res.unpinned.push(UnpinnedDependency {
            path: PathBuf::from("f.yml"),
            action: "a/b".into(),
            tag: Some("v1".into()),
            line: 1,
            column: 1,
        });
        assert!(!res.is_success());
    }
}
