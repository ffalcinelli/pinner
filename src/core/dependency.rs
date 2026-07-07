use serde::{Deserialize, Serialize};
use std::fmt;

/// Represents the CI/CD platform being used.
///
/// Each provider has its own YAML syntax and API for resolving dependencies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CiProvider {
    /// GitHub Actions (`uses`, `image`).
    GitHub,
    /// GitLab CI/CD (`include`, `image`, `ref`).
    GitLab,
    /// Bitbucket Pipelines (`pipe`, `image`).
    Bitbucket,
    /// CircleCI (`orbs`, `image`).
    CircleCI,
    /// Forgejo/Gitea Actions (`uses`, `image`).
    Forgejo,
    /// Gitea (similar to GitHub/Forgejo).
    Gitea,
    /// Azure DevOps Pipelines (`task`, `template`, `image`).
    AzureDevOps,
    /// AWS CodeBuild (`image`).
    AwsCodeBuild,
    /// Tekton Pipelines (`bundle`, `image`).
    Tekton,
    /// Kubernetes manifest (`image`).
    Kubernetes,
    /// Fallback for unknown or generic YAML files.
    #[default]
    Unknown,
}

/// Represents a dependency name.
///
/// This is a wrapper around a `String` representing the name of a CI/CD dependency,
/// such as a GitHub Action ("actions/checkout") or a Docker image ("alpine").
///
/// # Examples
/// ```
/// use pinner::core::DependencyName;
/// let name = DependencyName::from("actions/checkout");
/// assert_eq!(name.to_string(), "actions/checkout");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct DependencyName(pub String);

impl DependencyName {
    /// Returns true if the dependency is a Docker image (starts with docker://).
    pub fn is_docker(&self) -> bool {
        self.0.starts_with("docker://")
    }

    /// Returns the name without the "docker://" prefix if it exists.
    pub fn trim_docker_prefix(&self) -> &str {
        self.0.trim_start_matches("docker://")
    }

    /// Returns the repository owner and name part, i.e., "owner/repo" from "owner/repo/subdir".
    /// This is used for GitHub and Forgejo/Gitea repositories.
    pub fn repository_path(&self) -> &str {
        let trimmed = self.trim_docker_prefix();
        let mut parts = trimmed.splitn(3, '/');
        let owner = parts.next().unwrap_or("");
        let repo = parts.next().unwrap_or("");
        if repo.is_empty() {
            trimmed
        } else {
            let len = owner.len() + 1 + repo.len();
            &trimmed[..len]
        }
    }
}

impl fmt::Display for DependencyName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for DependencyName {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for DependencyName {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

/// Represents an immutable dependency reference.
///
/// Supports Git SHA-1 hashes, OCI/Docker container digests, and immutable versions.
///
/// # Examples
/// ```
/// use pinner::core::DependencyRef;
///
/// // Git SHA
/// let git_ref = DependencyRef::from("a1b2c3d4".to_string());
/// assert!(matches!(git_ref, DependencyRef::GitSha(_)));
///
/// // Docker Digest
/// let docker_ref = DependencyRef::from("sha256:abcdef...".to_string());
/// assert!(matches!(docker_ref, DependencyRef::DockerDigest(_)));
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DependencyRef {
    /// A Git commit SHA (usually 40 characters).
    GitSha(String),
    /// An OCI/Docker content digest (prefixed with `sha256:`).
    DockerDigest(String),
    /// An immutable semantic version (used for CircleCI Orbs).
    Version(String),
}

impl fmt::Display for DependencyRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GitSha(s) => write!(f, "{}", s),
            Self::DockerDigest(s) => write!(f, "{}", s),
            Self::Version(s) => write!(f, "{}", s),
        }
    }
}

impl From<String> for DependencyRef {
    fn from(s: String) -> Self {
        if s.starts_with("sha256:") {
            Self::DockerDigest(s)
        } else {
            Self::GitSha(s)
        }
    }
}

/// Represents a Git branch name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchName(pub String);

impl fmt::Display for BranchName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for BranchName {
    fn from(s: String) -> Self {
        Self(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dependency_name_docker() {
        let action = DependencyName::from("actions/checkout");
        assert!(!action.is_docker());
        assert_eq!(action.trim_docker_prefix(), "actions/checkout");
        assert_eq!(action.repository_path(), "actions/checkout");

        let action_sub = DependencyName::from("snyk/actions/setup");
        assert_eq!(action_sub.repository_path(), "snyk/actions");

        let action_sub2 = DependencyName::from("actions/aws/ecr/login");
        assert_eq!(action_sub2.repository_path(), "actions/aws");

        let docker = DependencyName::from("docker://alpine");
        assert!(docker.is_docker());
        assert_eq!(docker.trim_docker_prefix(), "alpine");
        assert_eq!(docker.repository_path(), "alpine");
    }

    #[test]
    fn test_dependency_ref_parsing() {
        let git_ref = DependencyRef::from("a1b2c3d4".to_string());
        assert!(matches!(git_ref, DependencyRef::GitSha(_)));
        assert_eq!(format!("{}", git_ref), "a1b2c3d4");

        let docker_ref = DependencyRef::from("sha256:abcdef".to_string());
        assert!(matches!(docker_ref, DependencyRef::DockerDigest(_)));
        assert_eq!(format!("{}", docker_ref), "sha256:abcdef");

        let ver_ref = DependencyRef::Version("1.2.3".to_string());
        assert_eq!(format!("{}", ver_ref), "1.2.3");
    }
}
