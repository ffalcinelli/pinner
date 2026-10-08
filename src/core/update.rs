use crate::core::dependency::{is_hash_ref, CiProvider, DependencyName, DependencyRef};
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
    /// Any consecutive block/header comments immediately preceding the dependency.
    pub preceding_comments: Option<String>,
    /// Tag written inline next to a digest in an image reference, e.g. `3.20` in
    /// `alpine:3.20@sha256:…`. The patcher keeps this `name:tag@digest` layout.
    pub image_tag: Option<String>,
    /// The YAML key used to define this dependency (e.g., `uses`, `image`, `pipe`).
    pub key: String,
    /// The CI provider detected for this task.
    pub provider: CiProvider,
}

/// Matches "version-only" comments such as `# v1`, `# main` or `# 1.2.3`.
///
/// The version token must be the whole comment or be followed by another `#`
/// (the `# v1 # note` layout pinner itself writes), so free-form comments like
/// `# mainly for X` or `# 3 retries` are never mistaken for versions.
///
/// Capture group 1 holds the version token. It is shared by the scanner (to recover
/// the logical tag of a pinned dependency) and the patcher (to replace a stale
/// version comment while keeping any extra annotation that follows it).
pub(crate) static VERSION_COMMENT_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^#\s*(v\d[a-zA-Z0-9.\-_+]*|main|\d[a-zA-Z0-9.\-_+]*)\s*(?:#|$)")
        .expect("Failed to compile VERSION_COMMENT_REGEX")
});

impl UpdateTask {
    /// Returns the logical tag of this dependency.
    /// If the current tag is a commit SHA or a Docker digest, it attempts to
    /// extract the tag from a trailing version comment (e.g., `# v1.2.3`).
    pub fn logical_tag(&self) -> Option<String> {
        let tag = self.current_tag.as_ref()?;
        if is_hash_ref(tag) {
            if let Some(comment) = &self.comment {
                if let Some(captures) = VERSION_COMMENT_REGEX.captures(comment) {
                    if let Some(m) = captures.get(1) {
                        return Some(m.as_str().to_string());
                    }
                }
            }
            if let Some(image_tag) = &self.image_tag {
                return Some(image_tag.clone());
            }
            if let Some(preceding) = &self.preceding_comments {
                for line in preceding.lines() {
                    let trimmed = line.trim();
                    if let Some(captures) = VERSION_COMMENT_REGEX.captures(trimmed) {
                        if let Some(m) = captures.get(1) {
                            return Some(m.as_str().to_string());
                        }
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

/// Details of a pinned container image that carries no cosign signature.
#[derive(Debug, Serialize, Clone)]
pub struct UnsignedDependency {
    /// Path to the file.
    pub path: PathBuf,
    /// Image name.
    pub action: DependencyName,
    /// The pinned digest.
    pub digest: String,
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
    /// Pinned images without a cosign signature (only checked with OSV checks enabled).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unsigned: Vec<UnsignedDependency>,
    /// Whether strict mode was enabled, which makes unsigned images fail verification.
    #[serde(skip)]
    pub strict: bool,
}

impl VerificationResult {
    /// Returns true if nothing is unpinned, compromised or non-vetted and, in strict
    /// mode, no image is unsigned.
    pub fn is_success(&self) -> bool {
        self.unpinned.is_empty()
            && self.compromised.is_empty()
            && self.non_vetted.is_empty()
            && (!self.strict || self.unsigned.is_empty())
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
    fn test_verification_result_unsigned_fails_only_when_strict() {
        let mut res = VerificationResult::default();
        res.unsigned.push(UnsignedDependency {
            path: PathBuf::from("f.yml"),
            action: "alpine".into(),
            digest: "sha256:abc".to_string(),
            line: 1,
            column: 1,
        });
        assert!(res.is_success());
        res.strict = true;
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

        // Free-form comments must not be mistaken for versions
        let sha = "de0fac2e4500dabe0009e67214ff5f5447ce83dd";
        for comment in [
            "# mainly for tests",
            "# 3 retries",
            "# v1 is broken upstream",
        ] {
            let task = UpdateTask {
                current_tag: Some(sha.to_string()),
                comment: Some(comment.to_string()),
                ..Default::default()
            };
            assert_eq!(task.logical_tag(), Some(sha.to_string()), "{comment}");
        }

        // Bare numeric image tags (e.g. node:20) are still versions
        let task = UpdateTask {
            current_tag: Some(format!("sha256:{}", "a".repeat(64))),
            comment: Some("# 20".to_string()),
            ..Default::default()
        };
        assert_eq!(task.logical_tag(), Some("20".to_string()));

        // Inline image tag next to a digest
        let task = UpdateTask {
            current_tag: Some(format!("sha256:{}", "a".repeat(64))),
            image_tag: Some("3.20".to_string()),
            ..Default::default()
        };
        assert_eq!(task.logical_tag(), Some("3.20".to_string()));
    }
}
