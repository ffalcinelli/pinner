use crate::core::dependency::{CiProvider, DependencyName, DependencyRef};
use serde::Serialize;
use std::path::PathBuf;

/// Represents a specific location in a file that needs to be updated.
#[derive(Debug, Clone, Default)]
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
    /// The current symbolic tag or ref (e.g., `v3`).
    pub current_tag: Option<String>,
    /// Any existing comment following the dependency on the same line.
    pub comment: Option<String>,
    /// The YAML key used to define this dependency (e.g., `uses`, `image`, `pipe`).
    pub key: String,
    /// The CI provider detected for this task.
    pub provider: CiProvider,
}

/// The result of a successful update resolution.
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

/// Machine-readable summary of updates.
#[derive(Serialize)]
pub struct JsonOutput {
    /// List of all successful updates.
    pub updates: Vec<UpdateResult>,
}

/// Details of a dependency that is not yet pinned to an immutable reference.
#[derive(Debug, Serialize, Clone)]
pub struct UnpinnedDependency {
    /// Path to the file.
    pub path: PathBuf,
    /// Action or image name.
    pub action: DependencyName,
    /// The current mutable tag.
    pub tag: Option<String>,
    /// Line number.
    pub line: usize,
    /// Column number.
    pub column: usize,
}

/// The result of a verification operation.
#[derive(Debug, Serialize, Clone, Default)]
pub struct VerificationResult {
    /// List of unpinned dependencies found.
    pub unpinned: Vec<UnpinnedDependency>,
}

impl VerificationResult {
    /// Returns true if no unpinned dependencies were found.
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

    #[test]
    fn test_update_result_serialization() {
        let res = UpdateResult {
            task: UpdateTask::default(), // Should be skipped
            action: "a/b".into(),
            path: PathBuf::from("f.yml"),
            old_tag: Some("v1".into()),
            new_sha: DependencyRef::GitSha("hash".into()),
            new_tag: Some("v1".into()),
        };

        let json = serde_json::to_string(&res).unwrap();
        assert!(!json.contains("task"));
        assert!(json.contains("\"action\":\"a/b\""));
    }
}
