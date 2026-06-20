use crate::core::dependency::{CiProvider, DependencyName, DependencyRef};
use regex::Regex;
use serde::Serialize;
use std::path::PathBuf;
use std::sync::LazyLock;

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

static VERSION_COMMENT_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^#\s*(v\d[a-zA-Z0-9.\-_]*|main|\d[a-zA-Z0-9.\-_]*)\s*")
        .expect("Failed to compile VERSION_COMMENT_REGEX")
});

impl UpdateTask {
    /// Returns the logical tag of this dependency.
    /// If the current tag is a commit SHA or a Docker digest, it attempts to
    /// extract the tag from a trailing version comment (e.g., `# v1.2.3`).
    pub fn logical_tag(&self) -> Option<String> {
        let tag = self.current_tag.as_ref()?;
        let is_sha = (tag.len() == 40 && tag.chars().all(|c| c.is_ascii_hexdigit()))
            || tag.starts_with("sha256:");
        if is_sha {
            if let Some(comment) = &self.comment {
                if let Some(captures) = VERSION_COMMENT_REGEX.captures(comment) {
                    if let Some(m) = captures.get(1) {
                        return Some(m.as_str().to_string());
                    }
                }
            }
        }
        Some(tag.clone())
    }
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

/// Details of a dependency that is pinned but marked as compromised.
#[derive(Debug, Serialize, Clone)]
pub struct CompromisedDependency {
    /// Path to the file.
    pub path: PathBuf,
    /// Action or image name.
    pub action: DependencyName,
    /// The compromised hash.
    pub hash: String,
    /// Line number.
    pub line: usize,
    /// Column number.
    pub column: usize,
}

/// Details of a dependency that is not explicitly vetted under strict mode.
#[derive(Debug, Serialize, Clone)]
pub struct NonVettedDependency {
    /// Path to the file.
    pub path: PathBuf,
    /// Action or image name.
    pub action: DependencyName,
    /// The hash or tag.
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
    /// List of compromised dependencies found.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub compromised: Vec<CompromisedDependency>,
    /// List of non-vetted dependencies found (only populated/checked in strict mode).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub non_vetted: Vec<NonVettedDependency>,
}

impl VerificationResult {
    /// Returns true if no unpinned, compromised, or non-vetted dependencies were found.
    pub fn is_success(&self) -> bool {
        self.unpinned.is_empty() && self.compromised.is_empty() && self.non_vetted.is_empty()
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
    fn test_verification_result_compromised() {
        let mut res = VerificationResult::default();
        res.compromised.push(CompromisedDependency {
            path: PathBuf::from("f.yml"),
            action: "a/b".into(),
            hash: "compromised_hash".to_string(),
            line: 1,
            column: 1,
        });
        assert!(!res.is_success());
    }

    #[test]
    fn test_verification_result_non_vetted() {
        let mut res = VerificationResult::default();
        res.non_vetted.push(NonVettedDependency {
            path: PathBuf::from("f.yml"),
            action: "a/b".into(),
            tag: Some("some_hash".to_string()),
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

    #[test]
    fn test_logical_tag() {
        // Tag is a normal version
        let task = UpdateTask {
            current_tag: Some("v3.1.2".to_string()),
            comment: None,
            ..Default::default()
        };
        assert_eq!(task.logical_tag(), Some("v3.1.2".to_string()));

        // Tag is a SHA but no comment
        let task = UpdateTask {
            current_tag: Some("de0fac2e4500dabe0009e67214ff5f5447ce83dd".to_string()),
            comment: None,
            ..Default::default()
        };
        assert_eq!(
            task.logical_tag(),
            Some("de0fac2e4500dabe0009e67214ff5f5447ce83dd".to_string())
        );

        // Tag is a SHA with a version comment
        let task = UpdateTask {
            current_tag: Some("de0fac2e4500dabe0009e67214ff5f5447ce83dd".to_string()),
            comment: Some("# v6.0.2".to_string()),
            ..Default::default()
        };
        assert_eq!(task.logical_tag(), Some("v6.0.2".to_string()));

        // Tag is a SHA with a version comment and other suffix
        let task = UpdateTask {
            current_tag: Some("de0fac2e4500dabe0009e67214ff5f5447ce83dd".to_string()),
            comment: Some("# v6.0.2 # keep me".to_string()),
            ..Default::default()
        };
        assert_eq!(task.logical_tag(), Some("v6.0.2".to_string()));
    }
}
