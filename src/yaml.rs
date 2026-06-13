use std::sync::LazyLock;
use tree_sitter::{Node, Query, QueryCursor};

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
        "#,
    )
    .expect("Failed to create tree-sitter query")
});

/// Finds all `uses` keys in the YAML AST using a declarative query.
pub fn find_uses_nodes(node: Node, content: &[u8], results: &mut Vec<(usize, usize, String)>) {
    let mut cursor = QueryCursor::new();
    let matches = cursor.matches(&USES_QUERY, node, content);

    let value_idx = USES_QUERY
        .capture_index_for_name("value")
        .expect("value capture missing");

    for m in matches {
        for cap in m.captures {
            if cap.index == value_idx {
                let mut v_node = cap.node;
                // Descend to the actual value node if nested
                while v_node.child_count() > 0 && v_node.kind() != "plain_scalar" {
                    if let Some(c) = v_node.child(0) {
                        v_node = c;
                    } else {
                        break;
                    }
                }

                results.push((
                    v_node.start_byte(),
                    v_node.end_byte(),
                    v_node.utf8_text(content).unwrap_or("").to_string(),
                ));
            }
        }
    }
}
