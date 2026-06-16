use serde::{Deserialize, Serialize};
use std::fmt;

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
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DependencyName(pub String);

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
/// Supports Git SHA-1 hashes and OCI/Docker container digests.
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
}

impl fmt::Display for DependencyRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GitSha(s) => write!(f, "{}", s),
            Self::DockerDigest(s) => write!(f, "{}", s),
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
