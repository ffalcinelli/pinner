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
          (#eq? @key "uses"))
        (comment) @comment
        "#,
    )
    .expect("Failed to create tree-sitter query")
});

/// Finds all `uses` keys and adjacent comments in the YAML AST.
pub fn find_uses_nodes(
    node: tree_sitter::Node,
    content: &[u8],
    results: &mut Vec<(usize, usize, String, Option<String>)>,
) {
    let mut cursor = QueryCursor::new();
    let matches = cursor.matches(&USES_QUERY, node, content);

    let value_idx = USES_QUERY
        .capture_index_for_name("value")
        .expect("value capture missing");
    let comment_idx = USES_QUERY.capture_index_for_name("comment");

    let mut last_value: Option<(usize, usize, String, usize)> = None;

    for m in matches {
        for cap in m.captures {
            if cap.index == value_idx {
                // If we have a pending value without a comment, push it
                if let Some((start, end, val, _)) = last_value.take() {
                    results.push((start, end, val, None));
                }

                let mut v_node = cap.node;
                while v_node.child_count() > 0 && v_node.kind() != "plain_scalar" {
                    if let Some(c) = v_node.child(0) {
                        v_node = c;
                    } else {
                        break;
                    }
                }

                last_value = Some((
                    v_node.start_byte(),
                    v_node.end_byte(),
                    v_node.utf8_text(content).unwrap_or("").to_string(),
                    v_node.start_position().row,
                ));
            } else if Some(cap.index) == comment_idx {
                if let Some((start, end, val, row)) = last_value.take() {
                    let comment_node = cap.node;
                    if comment_node.start_position().row == row {
                        let comment_text =
                            comment_node.utf8_text(content).unwrap_or("").to_string();
                        results.push((start, end, val, Some(comment_text)));
                    } else {
                        // Comment is on a different line, push the value without comment
                        results.push((start, end, val, None));
                        // The comment might be a top-level comment or related to something else,
                        // we don't have a value to pair it with right now.
                    }
                }
            }
        }
    }

    // Push the last value if it wasn't paired with a comment
    if let Some((start, end, val, _)) = last_value.take() {
        results.push((start, end, val, None));
    }
}
