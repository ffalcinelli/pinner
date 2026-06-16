use crate::core::UpdateTask;
use crate::core::{CiProvider, DependencyName};
use crate::error::PinnerError;
use std::path::Path;
use std::sync::LazyLock;
use tree_sitter::{Node, Point, Query, QueryCursor, StreamingIterator};

impl CiProvider {
    pub fn from_path(path: &Path) -> Self {
        let path_str = path.to_string_lossy();
        let mappings = [
            (".github/workflows", CiProvider::GitHub),
            (".forgejo/workflows", CiProvider::Forgejo),
            (".gitea/workflows", CiProvider::Gitea),
            (".gitlab-ci", CiProvider::GitLab),
            ("bitbucket-pipelines", CiProvider::Bitbucket),
            (".circleci", CiProvider::CircleCI),
            ("azure-pipelines", CiProvider::AzureDevOps),
            ("buildspec", CiProvider::AwsCodeBuild),
        ];

        for (pattern, provider) in mappings {
            if path_str.contains(pattern) {
                return provider;
            }
        }
        CiProvider::Unknown
    }

    pub fn supports_key(&self, key: &str) -> bool {
        match self {
            CiProvider::GitHub | CiProvider::Forgejo | CiProvider::Gitea => {
                matches!(key, "uses" | "image")
            }
            CiProvider::GitLab => matches!(key, "include" | "image" | "ref"),
            CiProvider::Bitbucket => matches!(key, "pipe" | "image"),
            // CircleCI support focuses exclusively on Docker Images (e.g. cimg/*)
            // as Orbs are strictly semantic versioned and do not support Git SHA pinning.
            CiProvider::CircleCI => matches!(key, "image"),
            CiProvider::AzureDevOps => matches!(key, "task" | "template" | "image"),
            CiProvider::AwsCodeBuild => matches!(key, "image"),
            CiProvider::Unknown => true,
        }
    }
}

static USES_QUERY: LazyLock<Result<Query, String>> = LazyLock::new(|| {
    Query::new(
        &tree_sitter_yaml::LANGUAGE.into(),
        r#"
        (block_mapping_pair
          key: [
            (flow_node (plain_scalar (string_scalar) @key))
            (plain_scalar (string_scalar) @key)
          ]
          value: (_) @value
          (#match? @key "^(uses|pipe|image|include|ref|task|template)$"))
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

fn resolve_gitlab_project(v_node: Node, content: &[u8]) -> Option<String> {
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

pub fn find_tasks(
    path: &Path,
    node: Node,
    content: &[u8],
    ignore_list: &[String],
) -> Result<Vec<UpdateTask>, PinnerError> {
    let mut results = Vec::new();
    let query = USES_QUERY
        .as_ref()
        .map_err(|e| PinnerError::Parse(e.clone()))?;

    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, node, content);

    let key_idx = query
        .capture_index_for_name("key")
        .ok_or_else(|| PinnerError::Parse("key capture missing".to_string()))?;
    let value_idx = query
        .capture_index_for_name("value")
        .ok_or_else(|| PinnerError::Parse("value capture missing".to_string()))?;
    let comment_idx = query.capture_index_for_name("comment");

    let provider = CiProvider::from_path(path);
    let ctx = FileContext {
        path,
        provider,
        ignore_list,
    };
    let mut last_value: Option<(usize, usize, String, Point, String)> = None;

    while let Some(m) = matches.next() {
        let mut current_key = String::new();
        for cap in m.captures {
            if cap.index == key_idx {
                current_key = cap.node.utf8_text(content).unwrap_or("").to_string();
            } else if cap.index == value_idx {
                if !provider.supports_key(&current_key) {
                    continue;
                }

                if let Some((start, end, value, pos, key)) = last_value.take() {
                    if let Some(task) = create_task(
                        ctx,
                        Position {
                            start,
                            end,
                            line: pos.row + 1,
                            column: pos.column + 1,
                        },
                        value,
                        None,
                        key,
                    ) {
                        results.push(task);
                    }
                }

                let v_node = cap.node;
                let mut val = unquote(v_node.utf8_text(content).unwrap_or(""));

                if current_key == "ref" {
                    if let Some(project) = resolve_gitlab_project(v_node, content) {
                        val = format!("{}@{}", project, val);
                    }
                }

                last_value = Some((
                    v_node.start_byte(),
                    v_node.end_byte(),
                    val,
                    v_node.start_position(),
                    current_key.clone(),
                ));
            } else if Some(cap.index) == comment_idx {
                if let Some((start, end, value, pos, key)) = last_value.take() {
                    let comment_node = cap.node;
                    if comment_node.start_position().row == pos.row {
                        let comment_text =
                            comment_node.utf8_text(content).unwrap_or("").to_string();
                        if let Some(task) = create_task(
                            ctx,
                            Position {
                                start,
                                end,
                                line: pos.row + 1,
                                column: pos.column + 1,
                            },
                            value,
                            Some(comment_text),
                            key,
                        ) {
                            results.push(task);
                        }
                    } else {
                        if let Some(task) = create_task(
                            ctx,
                            Position {
                                start,
                                end,
                                line: pos.row + 1,
                                column: pos.column + 1,
                            },
                            value,
                            None,
                            key,
                        ) {
                            results.push(task);
                        }
                    }
                }
            }
        }
    }

    if let Some((start, end, value, pos, key)) = last_value {
        if let Some(task) = create_task(
            ctx,
            Position {
                start,
                end,
                line: pos.row + 1,
                column: pos.column + 1,
            },
            value,
            None,
            key,
        ) {
            results.push(task);
        }
    }
    Ok(results)
}

#[derive(Clone, Copy)]
struct FileContext<'a> {
    path: &'a Path,
    provider: CiProvider,
    ignore_list: &'a [String],
}

struct Position {
    start: usize,
    end: usize,
    line: usize,
    column: usize,
}

fn create_task(
    ctx: FileContext,
    pos: Position,
    value: String,
    comment: Option<String>,
    key: String,
) -> Option<UpdateTask> {
    if key == "include" || key == "project" {
        return None;
    }
    if value.starts_with("./") {
        return None;
    }

    let (action_part, tag) = if let Some((a, t)) = value.split_once('@') {
        (a, Some(t))
    } else if value.starts_with("docker://") && value.contains(':') {
        if let Some(last_colon) = value.rfind(':') {
            (&value[..last_colon], Some(&value[last_colon + 1..]))
        } else {
            (value.as_str(), None)
        }
    } else if let Some((a, t)) = value.split_once(':') {
        (a, Some(t))
    } else {
        (value.as_str(), None)
    };

    let action = DependencyName::from(action_part);

    if ctx
        .ignore_list
        .iter()
        .any(|pattern| action.0.contains(pattern))
    {
        return None;
    }

    Some(UpdateTask {
        path: ctx.path.to_path_buf(),
        start: pos.start,
        end: pos.end,
        line: pos.line,
        column: pos.column,
        action,
        current_tag: tag.map(|s| s.to_string()),
        comment,
        key,
        provider: ctx.provider,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tree_sitter::Parser as TSParser;

    fn parse_yaml(content: &str) -> (tree_sitter::Tree, Vec<u8>) {
        let mut parser = TSParser::new();
        parser
            .set_language(&tree_sitter_yaml::LANGUAGE.into())
            .expect("Error loading YAML grammar");
        let tree = parser.parse(content, None).expect("Error parsing YAML");
        (tree, content.as_bytes().to_vec())
    }

    #[test]
    fn test_find_tasks_github() {
        let yaml = "uses: actions/checkout@v3";
        let (tree, content) = parse_yaml(yaml);
        let path = Path::new(".github/workflows/ci.yml");
        let results = find_tasks(path, tree.root_node(), &content, &[]).unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].action.0, "actions/checkout");
        assert_eq!(results[0].current_tag.as_deref(), Some("v3"));
        assert_eq!(results[0].key, "uses");
    }

    #[test]
    fn test_find_tasks_with_quotes() {
        let yaml = "uses: \"actions/checkout@v3\"";
        let (tree, content) = parse_yaml(yaml);
        let path = Path::new(".github/workflows/ci.yml");
        let results = find_tasks(path, tree.root_node(), &content, &[]).unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].action.0, "actions/checkout");
        assert_eq!(results[0].current_tag.as_deref(), Some("v3"));
    }

    #[test]
    fn test_find_tasks_with_comment() {
        let yaml = "uses: actions/checkout@hash # v3";
        let (tree, content) = parse_yaml(yaml);
        let path = Path::new(".github/workflows/ci.yml");
        let results = find_tasks(path, tree.root_node(), &content, &[]).unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].action.0, "actions/checkout");
        assert_eq!(results[0].current_tag.as_deref(), Some("hash"));
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
        let path = Path::new("other.yml");
        let results = find_tasks(path, tree.root_node(), &content, &[]).unwrap();

        let keys: Vec<String> = results.iter().map(|r| r.key.clone()).collect();
        assert!(keys.contains(&"image".to_string()));
        assert!(keys.contains(&"pipe".to_string()));
    }

    fn find_node_with_text<'a>(
        node: tree_sitter::Node<'a>,
        text: &str,
        content: &[u8],
    ) -> Option<tree_sitter::Node<'a>> {
        if node.utf8_text(content).unwrap_or("") == text {
            return Some(node);
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if let Some(found) = find_node_with_text(child, text, content) {
                return Some(found);
            }
        }
        None
    }

    #[test]
    fn test_resolve_gitlab_project_success() {
        let yaml = r#"
include:
  - project: 'my-group/my-project'
    ref: 'v1.0.0'
"#;
        let (tree, content) = parse_yaml(yaml);
        let node = find_node_with_text(tree.root_node(), "'v1.0.0'", &content).unwrap();
        let project = resolve_gitlab_project(node, &content);
        assert_eq!(project, Some("my-group/my-project".to_string()));
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
        let path = Path::new(".gitlab-ci.yml");
        let results = find_tasks(path, tree.root_node(), &content, &[]).unwrap();

        let ref_node = results.iter().find(|r| r.key == "ref").unwrap();
        assert_eq!(ref_node.action.0, "my-group/my-project");
        assert_eq!(ref_node.current_tag.as_deref(), Some("v1.0.0"));
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
        let path = Path::new(".github/workflows/ci.yml");
        let results = find_tasks(path, tree.root_node(), &content, &[]).unwrap();

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
    fn test_ci_provider_from_path() {
        assert_eq!(
            CiProvider::from_path(Path::new(".github/workflows/ci.yml")),
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
    }

    #[test]
    fn test_find_tasks_azure_devops() {
        let yaml = r#"
steps:
- task: NodeTool@0
  inputs:
    versionSpec: '16.x'
- template: templates/build.yml@templates-repo
  parameters:
    buildConfig: 'Release'
"#;
        let (tree, content) = parse_yaml(yaml);
        let path = Path::new("azure-pipelines.yml");
        let results = find_tasks(path, tree.root_node(), &content, &[]).unwrap();

        assert_eq!(results.len(), 2);

        let task = results.iter().find(|r| r.key == "task").unwrap();
        assert_eq!(task.action.0, "NodeTool");
        assert_eq!(task.current_tag.as_deref(), Some("0"));

        let template = results.iter().find(|r| r.key == "template").unwrap();
        assert_eq!(template.action.0, "templates/build.yml");
        assert_eq!(template.current_tag.as_deref(), Some("templates-repo"));
    }

    #[test]
    fn test_find_tasks_aws_codebuild() {
        let yaml = r#"
version: 0.2
phases:
  install:
    runtime-versions:
      nodejs: 16
build:
  commands:
    - echo "Building..."
image: aws/codebuild/standard:5.0
"#;
        let (tree, content) = parse_yaml(yaml);
        let path = Path::new("buildspec.yml");
        let results = find_tasks(path, tree.root_node(), &content, &[]).unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].action.0, "aws/codebuild/standard");
        assert_eq!(results[0].current_tag.as_deref(), Some("5.0"));
    }
}
