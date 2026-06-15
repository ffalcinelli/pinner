//! YAML parsing utilities using `tree-sitter`.
//!
//! This module provides functions to traverse the YAML concrete syntax tree
//! and identify nodes that represent CI/CD dependencies (like GitHub Actions
//! or Docker images). It uses a pre-compiled tree-sitter query to find
//! relevant keys like `uses`, `image`, and `pipe`.

use std::path::Path;
use std::sync::LazyLock;
use tree_sitter::{Query, QueryCursor};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CiProvider {
    GitHub,
    GitLab,
    Bitbucket,
    CircleCI,
    Unknown,
}

impl CiProvider {
    pub fn from_path(path: &Path) -> Self {
        let path_str = path.to_string_lossy();
        if path_str.contains(".github/workflows") || path_str.contains(".gitea/workflows") {
            CiProvider::GitHub
        } else if path_str.contains(".gitlab-ci") {
            CiProvider::GitLab
        } else if path_str.contains("bitbucket-pipelines") {
            CiProvider::Bitbucket
        } else if path_str.contains(".circleci") {
            CiProvider::CircleCI
        } else {
            CiProvider::Unknown
        }
    }

    pub fn supports_key(&self, key: &str) -> bool {
        match self {
            CiProvider::GitHub => matches!(key, "uses" | "image"),
            CiProvider::GitLab => matches!(key, "include" | "image" | "ref"),
            CiProvider::Bitbucket => matches!(key, "pipe" | "image"),
            CiProvider::CircleCI => matches!(key, "orbs" | "image"),
            CiProvider::Unknown => true,
        }
    }
}

/// Represents a dependency reference found in a YAML file.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct DependencyNode {
    /// Starting byte offset in the content.
    pub start: usize,
    /// Ending byte offset in the content.
    pub end: usize,
    /// The unquoted value of the dependency (e.g., "actions/checkout@v3").
    pub value: String,
    /// Optional adjacent comment (e.g., "# v3").
    pub comment: Option<String>,
    /// The YAML key where the dependency was found (e.g., "uses").
    pub key: String,
}

static USES_QUERY: LazyLock<Result<Query, String>> = LazyLock::new(|| {
    Query::new(
        tree_sitter_yaml::language(),
        r#"
        (block_mapping_pair
          key: [
            (flow_node (plain_scalar (string_scalar) @key))
            (plain_scalar (string_scalar) @key)
          ]
          value: (_) @value
          (#match? @key "^(uses|pipe|image|include|ref|orbs)$"))
        (comment) @comment
        "#,
    )
    .map_err(|e| format!("Failed to create tree-sitter query: {:?}", e))
});

fn unquote(s: &str) -> String {
    let s = s.trim();
    if ((s.starts_with('\'') && s.ends_with('\'')) || (s.starts_with('"') && s.ends_with('"')))
        && s.len() >= 2
    {
        return s[1..s.len() - 1].to_string();
    }
    s.to_string()
}

fn resolve_gitlab_project(v_node: tree_sitter::Node, content: &[u8]) -> Option<String> {
    let parent_pair = v_node.parent()?;
    let mapping = parent_pair.parent()?;
    let mut cursor = mapping.walk();
    for child in mapping.children(&mut cursor) {
        if child.kind() == "block_mapping_pair" {
            if let Some(k_node) = child.child_by_field_name("key") {
                if k_node.utf8_text(content).unwrap_or("") == "project" {
                    if let Some(v_node) = child.child_by_field_name("value") {
                        return Some(unquote(v_node.utf8_text(content).unwrap_or("")));
                    }
                }
            }
        }
    }
    None
}

/// Finds all `uses`, `pipe`, `image`, `include`, `ref`, or `orbs` keys and adjacent comments in the YAML AST.
pub fn find_uses_nodes(
    node: tree_sitter::Node,
    content: &[u8],
    provider: CiProvider,
) -> Result<Vec<DependencyNode>, crate::error::PinnerError> {
    let mut results = Vec::new();
    let query = USES_QUERY
        .as_ref()
        .map_err(|e| crate::error::PinnerError::Parse(e.clone()))?;

    let mut cursor = QueryCursor::new();
    let matches = cursor.matches(query, node, content);

    let key_idx = query
        .capture_index_for_name("key")
        .ok_or_else(|| crate::error::PinnerError::Parse("key capture missing".to_string()))?;
    let value_idx = query
        .capture_index_for_name("value")
        .ok_or_else(|| crate::error::PinnerError::Parse("value capture missing".to_string()))?;
    let comment_idx = query.capture_index_for_name("comment");

    let mut last_value: Option<(usize, usize, String, usize, String)> = None;

    for m in matches {
        let mut current_key = String::new();
        for cap in m.captures {
            if cap.index == key_idx {
                current_key = cap.node.utf8_text(content).unwrap_or("").to_string();
            } else if cap.index == value_idx {
                if !provider.supports_key(&current_key) {
                    continue;
                }
                // If we have a pending value without a comment, push it
                if let Some((start, end, value, _, key)) = last_value.take() {
                    results.push(DependencyNode {
                        start,
                        end,
                        value,
                        comment: None,
                        key,
                    });
                }

                let v_node = cap.node;
                let mut val = unquote(v_node.utf8_text(content).unwrap_or(""));

                // GitLab context: if we found a 'ref', try to find a sibling 'project'
                if current_key == "ref" {
                    if let Some(project) = resolve_gitlab_project(v_node, content) {
                        val = format!("{}@{}", project, val);
                    }
                }

                last_value = Some((
                    v_node.start_byte(),
                    v_node.end_byte(),
                    val,
                    v_node.start_position().row,
                    current_key.clone(),
                ));
            } else if Some(cap.index) == comment_idx {
                if let Some((start, end, value, row, key)) = last_value.take() {
                    let comment_node = cap.node;
                    if comment_node.start_position().row == row {
                        let comment_text =
                            comment_node.utf8_text(content).unwrap_or("").to_string();
                        results.push(DependencyNode {
                            start,
                            end,
                            value,
                            comment: Some(comment_text),
                            key,
                        });
                    } else {
                        results.push(DependencyNode {
                            start,
                            end,
                            value,
                            comment: None,
                            key,
                        });
                        // The comment might belong to the NEXT value, but we can't be sure here
                        // For now, we only support same-line comments for action tags.
                    }
                }
            }
        }
    }

    if let Some((start, end, value, _, key)) = last_value {
        results.push(DependencyNode {
            start,
            end,
            value,
            comment: None,
            key,
        });
    }
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tree_sitter::Parser as TSParser;

    fn parse_yaml(content: &str) -> (tree_sitter::Tree, Vec<u8>) {
        let mut parser = TSParser::new();
        parser
            .set_language(tree_sitter_yaml::language())
            .expect("Error loading YAML grammar");
        let tree = parser.parse(content, None).expect("Error parsing YAML");
        (tree, content.as_bytes().to_vec())
    }

    #[test]
    fn test_find_uses_nodes() {
        let yaml = "uses: actions/checkout@v3";
        let (tree, content) = parse_yaml(yaml);
        let results = find_uses_nodes(tree.root_node(), &content, CiProvider::GitHub).unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].value, "actions/checkout@v3");
        assert_eq!(results[0].key, "uses");
    }

    #[test]
    fn test_find_uses_with_quotes() {
        let yaml = "uses: \"actions/checkout@v3\"";
        let (tree, content) = parse_yaml(yaml);
        let results = find_uses_nodes(tree.root_node(), &content, CiProvider::GitHub).unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].value, "actions/checkout@v3");
    }

    #[test]
    fn test_find_uses_with_comment() {
        let yaml = "uses: actions/checkout@hash # v3";
        let (tree, content) = parse_yaml(yaml);
        let results = find_uses_nodes(tree.root_node(), &content, CiProvider::GitHub).unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].value, "actions/checkout@hash");
        assert_eq!(results[0].comment, Some("# v3".to_string()));
    }

    #[test]
    fn test_find_other_keys() {
        let yaml = r#"
image: alpine:latest
pipe: sonarsource/sonarcloud-scan:1.4.0
include: other-template.yml
orbs:
  node: circleci/node@5.0.0
"#;
        let (tree, content) = parse_yaml(yaml);
        let results = find_uses_nodes(tree.root_node(), &content, CiProvider::Unknown).unwrap();

        let keys: Vec<String> = results.iter().map(|r| r.key.clone()).collect();
        assert!(keys.contains(&"image".to_string()));
        assert!(keys.contains(&"pipe".to_string()));
        assert!(keys.contains(&"include".to_string()));
        assert!(keys.contains(&"orbs".to_string()));
    }

    #[test]
    fn test_gitlab_ref_project() {
        let yaml = r#"
include:
  - project: 'my-group/my-project'
    ref: 'v1.0.0'
    file: '/templates/.gitlab-ci.yml'
"#;
        let (tree, content) = parse_yaml(yaml);
        let results = find_uses_nodes(tree.root_node(), &content, CiProvider::GitLab).unwrap();

        // We expect "my-group/my-project@v1.0.0" for the 'ref' key
        let ref_node = results.iter().find(|r| r.key == "ref").unwrap();
        assert_eq!(ref_node.value, "my-group/my-project@v1.0.0");
    }

    #[test]
    fn test_github_ignore_include() {
        let yaml = r#"
strategy:
  matrix:
    include:
      - os: ubuntu-latest
"#;
        let (tree, content) = parse_yaml(yaml);
        let results = find_uses_nodes(tree.root_node(), &content, CiProvider::GitHub).unwrap();

        assert!(results.is_empty());
    }

    #[test]
    fn test_unquote_exhaustive() {
        assert_eq!(unquote("'v1'"), "v1");
        assert_eq!(unquote("\"v2\""), "v2");
        assert_eq!(unquote("v3"), "v3");
        assert_eq!(unquote("'"), "'");
        assert_eq!(unquote("\""), "\"");
        assert_eq!(unquote("''"), "");
        assert_eq!(unquote("  'v4'  "), "v4");
    }

    #[test]
    fn test_comment_on_different_line() {
        let yaml = r#"
uses: actions/checkout@v3
# unrelated comment
"#;
        let (tree, content) = parse_yaml(yaml);
        let results = find_uses_nodes(tree.root_node(), &content, CiProvider::GitHub).unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].comment, None);
    }

    #[test]
    fn test_ci_provider_from_path() {
        assert_eq!(
            CiProvider::from_path(Path::new(".github/workflows/ci.yml")),
            CiProvider::GitHub
        );
        assert_eq!(
            CiProvider::from_path(Path::new(".gitea/workflows/deploy.yaml")),
            CiProvider::GitHub
        );
        assert_eq!(
            CiProvider::from_path(Path::new(".gitlab-ci.yml")),
            CiProvider::GitLab
        );
        assert_eq!(
            CiProvider::from_path(Path::new("bitbucket-pipelines.yml")),
            CiProvider::Bitbucket
        );
        assert_eq!(
            CiProvider::from_path(Path::new(".circleci/config.yml")),
            CiProvider::CircleCI
        );
        assert_eq!(
            CiProvider::from_path(Path::new("docker-compose.yml")),
            CiProvider::Unknown
        );
    }

    #[test]
    fn test_ci_provider_supports_key() {
        let github = CiProvider::GitHub;
        assert!(github.supports_key("uses"));
        assert!(github.supports_key("image"));
        assert!(!github.supports_key("pipe"));

        let gitlab = CiProvider::GitLab;
        assert!(gitlab.supports_key("include"));
        assert!(gitlab.supports_key("ref"));
        assert!(!gitlab.supports_key("uses"));

        let bitbucket = CiProvider::Bitbucket;
        assert!(bitbucket.supports_key("pipe"));
        assert!(bitbucket.supports_key("image"));
        assert!(!bitbucket.supports_key("orbs"));

        let circle = CiProvider::CircleCI;
        assert!(circle.supports_key("orbs"));
        assert!(circle.supports_key("image"));
        assert!(!circle.supports_key("include"));

        let unknown = CiProvider::Unknown;
        assert!(unknown.supports_key("any_key"));
    }
}
