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
                let mut val = v_node.utf8_text(content).unwrap_or("").trim().to_string();

                // Remove surrounding quotes if present
                if (val.starts_with('\'') && val.ends_with('\''))
                    || (val.starts_with('"') && val.ends_with('"'))
                {
                    if val.len() >= 2 {
                        val = val[1..val.len() - 1].to_string();
                    }
                }

                // GitLab context: if we found a 'ref', try to find a sibling 'project'
                if current_key == "ref" {
                    if let Some(parent_pair) = v_node.parent() {
                        if let Some(mapping) = parent_pair.parent() {
                            let mut cursor = mapping.walk();
                            for child in mapping.children(&mut cursor) {
                                if child.kind() == "block_mapping_pair" {
                                    if let Some(k_node) = child.child_by_field_name("key") {
                                        if k_node.utf8_text(content).unwrap_or("") == "project" {
                                            if let Some(v_node) = child.child_by_field_name("value")
                                            {
                                                let project = v_node
                                                    .utf8_text(content)
                                                    .unwrap_or("")
                                                    .trim_matches('\'')
                                                    .trim_matches('"');
                                                val = format!("{}@{}", project, val);
                                            }
                                        }
                                    }
                                }
                            }
                        }
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
                        // The comment might be a top-level comment or related to something else,
                        // we don't have a value to pair it with right now.
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
