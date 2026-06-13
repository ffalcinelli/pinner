//! YAML parsing utilities using `tree-sitter`.
//!
//! This module provides functions to traverse the YAML concrete syntax tree
//! and identify nodes that represent CI/CD dependencies (like GitHub Actions
//! or Docker images). It uses a pre-compiled tree-sitter query to find
//! relevant keys like `uses`, `image`, and `pipe`.

use std::sync::LazyLock;
use tree_sitter::{Query, QueryCursor};

static USES_QUERY: LazyLock<Query> = LazyLock::new(|| {
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
    .expect("Failed to create tree-sitter query")
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
    results: &mut Vec<(usize, usize, String, Option<String>, String)>,
) {
    let mut cursor = QueryCursor::new();
    let matches = cursor.matches(&USES_QUERY, node, content);

    let key_idx = USES_QUERY
        .capture_index_for_name("key")
        .expect("key capture missing");
    let value_idx = USES_QUERY
        .capture_index_for_name("value")
        .expect("value capture missing");
    let comment_idx = USES_QUERY.capture_index_for_name("comment");

    let mut last_value: Option<(usize, usize, String, usize, String)> = None;

    for m in matches {
        let mut current_key = String::new();
        for cap in m.captures {
            if cap.index == key_idx {
                current_key = cap.node.utf8_text(content).unwrap_or("").to_string();
            } else if cap.index == value_idx {
                // If we have a pending value without a comment, push it
                if let Some((start, end, val, _, key)) = last_value.take() {
                    results.push((start, end, val, None, key));
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
                if let Some((start, end, val, row, key)) = last_value.take() {
                    let comment_node = cap.node;
                    if comment_node.start_position().row == row {
                        let comment_text =
                            comment_node.utf8_text(content).unwrap_or("").to_string();
                        results.push((start, end, val, Some(comment_text), key));
                    } else {
                        // Comment is on a different line, push the value without comment
                        results.push((start, end, val, None, key));
                    }
                }
            }
        }
    }

    // Push the last value if it wasn't paired with a comment
    if let Some((start, end, val, _, key)) = last_value.take() {
        results.push((start, end, val, None, key));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tree_sitter::Parser;

    fn parse_yaml(content: &str) -> (tree_sitter::Tree, Vec<u8>) {
        let mut parser = Parser::new();
        parser
            .set_language(tree_sitter_yaml::language())
            .expect("Error loading YAML grammar");
        let content_bytes = content.as_bytes().to_vec();
        let tree = parser.parse(&content_bytes, None).unwrap();
        (tree, content_bytes)
    }

    #[test]
    fn test_find_uses_basic() {
        let yaml = "uses: actions/checkout@v3";
        let (tree, content) = parse_yaml(yaml);
        let mut results = Vec::new();
        find_uses_nodes(tree.root_node(), &content, &mut results);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].2, "actions/checkout@v3");
        assert_eq!(results[0].3, None);
        assert_eq!(results[0].4, "uses");
    }

    #[test]
    fn test_find_uses_with_quotes() {
        let yaml = "uses: \"actions/checkout@v3\"";
        let (tree, content) = parse_yaml(yaml);
        let mut results = Vec::new();
        find_uses_nodes(tree.root_node(), &content, &mut results);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].2, "actions/checkout@v3");
    }

    #[test]
    fn test_find_uses_with_comment() {
        let yaml = "uses: actions/checkout@hash # v3";
        let (tree, content) = parse_yaml(yaml);
        let mut results = Vec::new();
        find_uses_nodes(tree.root_node(), &content, &mut results);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].2, "actions/checkout@hash");
        assert_eq!(results[0].3, Some("# v3".to_string()));
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
        let mut results = Vec::new();
        find_uses_nodes(tree.root_node(), &content, &mut results);

        let keys: Vec<String> = results.iter().map(|r| r.4.clone()).collect();
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
        let mut results = Vec::new();
        find_uses_nodes(tree.root_node(), &content, &mut results);

        // We expect "my-group/my-project@v1.0.0" for the 'ref' key
        let ref_node = results.iter().find(|r| r.4 == "ref").unwrap();
        assert_eq!(ref_node.2, "my-group/my-project@v1.0.0");
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
        let mut results = Vec::new();
        find_uses_nodes(tree.root_node(), &content, &mut results);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].3, None);
    }
}
