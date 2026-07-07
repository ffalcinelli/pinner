use crate::core::UpdateTask;
use crate::core::{CiProvider, DependencyName};
use crate::error::PinnerError;
use globset::Glob;
use std::path::Path;
use std::sync::LazyLock;
use tree_sitter::{Node, Point, Query, QueryCursor, StreamingIterator};

impl CiProvider {
    /// Detects the CI provider based on the file path.
    ///
    /// This heuristic matches standard CI/CD directory structures (e.g., `.github/workflows`).
    pub fn from_path(path: &Path) -> Self {
        let path_str = path.to_string_lossy().to_lowercase();
        let mappings = [
            (".github/workflows", CiProvider::GitHub),
            (".forgejo/workflows", CiProvider::Forgejo),
            (".gitea/workflows", CiProvider::Gitea),
            (".gitlab-ci", CiProvider::GitLab),
            ("bitbucket-pipelines", CiProvider::Bitbucket),
            (".circleci", CiProvider::CircleCI),
            ("azure-pipelines", CiProvider::AzureDevOps),
            ("buildspec", CiProvider::AwsCodeBuild),
            ("tekton", CiProvider::Tekton),
            ("kubernetes", CiProvider::Kubernetes),
            ("k8s", CiProvider::Kubernetes),
        ];

        for (pattern, provider) in mappings {
            if path_str.contains(pattern) {
                return provider;
            }
        }

        // Check common Kubernetes manifest file names
        if let Some(file_name) = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.to_lowercase())
        {
            if file_name == "pod.yaml"
                || file_name == "pod.yml"
                || file_name == "deployment.yaml"
                || file_name == "deployment.yml"
                || file_name == "statefulset.yaml"
                || file_name == "statefulset.yml"
                || file_name == "daemonset.yaml"
                || file_name == "daemonset.yml"
                || file_name == "replicaset.yaml"
                || file_name == "replicaset.yml"
                || file_name == "job.yaml"
                || file_name == "job.yml"
                || file_name == "cronjob.yaml"
                || file_name == "cronjob.yml"
            {
                return CiProvider::Kubernetes;
            }
        }

        CiProvider::Unknown
    }

    /// Returns true if the given YAML key represents a dependency for this provider.
    ///
    /// For example, GitHub Actions uses the `uses` key, while Bitbucket Pipelines uses `pipe`.
    pub fn supports_key(&self, key: &str) -> bool {
        match self {
            CiProvider::GitHub | CiProvider::Forgejo | CiProvider::Gitea => {
                matches!(key, "uses" | "image")
            }
            CiProvider::GitLab => matches!(key, "include" | "image" | "ref"),
            CiProvider::Bitbucket => matches!(key, "pipe" | "image"),
            // CircleCI support includes Docker Images (e.g. cimg/*) and Orbs.
            CiProvider::CircleCI => matches!(key, "image" | "orbs"),
            CiProvider::AzureDevOps => matches!(key, "task" | "template" | "image"),
            CiProvider::AwsCodeBuild => matches!(key, "image"),
            CiProvider::Tekton => matches!(key, "bundle" | "image"),
            CiProvider::Kubernetes => matches!(key, "image"),
            CiProvider::Unknown => true,
        }
    }
}

/// Tree-sitter query to identify potential dependency nodes in YAML.
///
/// It targets common keys like `uses`, `image`, `pipe`, etc., and also captures
/// the specific structure of CircleCI Orbs. Comments are captured separately
/// to associate them with the preceding value if they appear on the same line.
static USES_QUERY: LazyLock<Result<Query, String>> = LazyLock::new(|| {
    Query::new(
        &tree_sitter_yaml::LANGUAGE.into(),
        r#"
        ; Capture standard key-value pairs where the key matches our known dependency triggers.
        (block_mapping_pair
          key: [
            (flow_node (plain_scalar (string_scalar) @key))
            (plain_scalar (string_scalar) @key)
          ]
          value: (_) @value
          (#match? @key "^(uses|pipe|image|include|ref|task|template|bundle)$"))

        ; Capture CircleCI Orbs which have a nested structure: orbs -> name -> value.
        (block_mapping_pair
          key: [
            (flow_node (plain_scalar (string_scalar) @key))
            (plain_scalar (string_scalar) @key)
          ]
          (#eq? @key "orbs")
          value: (block_node
            (block_mapping
              (block_mapping_pair
                value: [
                  (flow_node (plain_scalar (string_scalar) @value))
                  (plain_scalar (string_scalar) @value)
                ]
              )
            )
          )
        )

        ; Capture comments to associate them with the value node above them.
        (comment) @comment
        "#,
    )
    .map_err(|e| format!("Failed to create tree-sitter query: {:?}", e))
});

/// Removes surrounding quotes from a string.
fn unquote(s: &str) -> String {
    let s = s.trim();
    if ((s.starts_with('\'') && s.ends_with('\'')) || (s.starts_with('"') && s.ends_with('"')))
        && s.len() >= 2
    {
        return s[1..s.len() - 1].to_string();
    }
    s.to_string()
}

/// Resolves the GitLab project name for an `include` entry.
///
/// In GitLab CI, an `include` can specify a `project` and a `ref`.
/// This function walks the AST to find the sibling `project` key for a given `ref` value.
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

/// Identifies all dependency update tasks within a YAML AST node.
///
/// This function uses tree-sitter queries to find relevant keys (like `uses`, `image`, `orbs`)
/// and maps them to `UpdateTask` domain models. It handles provider-specific logic
/// and associates end-of-line comments with their respective values.
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

    let content_str = std::str::from_utf8(content).unwrap_or("");
    let lines: Vec<&str> = content_str.lines().collect();

    // We keep track of the last value found to associate it with a potential comment
    // on the same line. Because tree-sitter queries can return captures in sequence,
    // we "buffer" the value until we see if a comment follows it on the same line.
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

                // If we had a buffered value from a previous match that didn't have a same-line comment,
                // push it now.
                if let Some((start, end, value, pos, key)) = last_value.take() {
                    let task_line = pos.row + 1;
                    let preceding_comments = collect_preceding_comments(&lines, task_line);
                    if let Some(mut task) = create_task(
                        ctx,
                        Position {
                            start,
                            end,
                            line: task_line,
                            column: pos.column + 1,
                        },
                        value,
                        None,
                        key,
                    ) {
                        task.preceding_comments = preceding_comments;
                        results.push(task);
                    }
                }

                let v_node = cap.node;
                let mut val = unquote(v_node.utf8_text(content).unwrap_or(""));

                // GitLab special case: combine 'project' and 'ref' into a single virtual dependency.
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
                    let task_line = pos.row + 1;
                    let preceding_comments = collect_preceding_comments(&lines, task_line);
                    // Check if the comment is on the same line as the buffered value.
                    if comment_node.start_position().row == pos.row {
                        let comment_text =
                            comment_node.utf8_text(content).unwrap_or("").to_string();
                        if let Some(mut task) = create_task(
                            ctx,
                            Position {
                                start,
                                end,
                                line: task_line,
                                column: pos.column + 1,
                            },
                            value,
                            Some(comment_text),
                            key,
                        ) {
                            task.preceding_comments = preceding_comments;
                            results.push(task);
                        }
                    } else {
                        // The comment is on a different line, so the buffered value has no comment.
                        if let Some(mut task) = create_task(
                            ctx,
                            Position {
                                start,
                                end,
                                line: task_line,
                                column: pos.column + 1,
                            },
                            value,
                            None,
                            key,
                        ) {
                            task.preceding_comments = preceding_comments;
                            results.push(task);
                        }
                    }
                }
            }
        }
    }

    if let Some((start, end, value, pos, key)) = last_value {
        let task_line = pos.row + 1;
        let preceding_comments = collect_preceding_comments(&lines, task_line);
        if let Some(mut task) = create_task(
            ctx,
            Position {
                start,
                end,
                line: task_line,
                column: pos.column + 1,
            },
            value,
            None,
            key,
        ) {
            task.preceding_comments = preceding_comments;
            results.push(task);
        }
    }
    Ok(results)
}

fn collect_preceding_comments(lines: &[&str], line: usize) -> Option<String> {
    if line < 2 {
        return None;
    }

    let mut collected = Vec::new();
    let mut curr_idx = line - 2;

    loop {
        let trimmed = lines[curr_idx].trim();
        if trimmed.starts_with('#') {
            collected.push(trimmed.to_string());
            if curr_idx == 0 {
                break;
            }
            curr_idx -= 1;
        } else {
            break;
        }
    }

    if collected.is_empty() {
        None
    } else {
        collected.reverse();
        Some(collected.join("\n"))
    }
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
    } else {
        let is_docker = value.starts_with("docker://");
        let path_to_check = if is_docker {
            value.strip_prefix("docker://").unwrap()
        } else {
            value.as_str()
        };

        if let Some(last_colon) = path_to_check.rfind(':') {
            let after_colon = &path_to_check[last_colon + 1..];
            if after_colon.contains('/') {
                (value.as_str(), None)
            } else {
                let split_idx = if is_docker {
                    last_colon + "docker://".len()
                } else {
                    last_colon
                };
                (&value[..split_idx], Some(&value[split_idx + 1..]))
            }
        } else {
            (value.as_str(), None)
        }
    };

    let action = DependencyName::from(action_part);

    let mut ignored = false;
    for pattern in ctx.ignore_list {
        let has_wildcards = pattern
            .chars()
            .any(|c| matches!(c, '*' | '?' | '[' | ']' | '{' | '}'));
        if has_wildcards {
            if let Ok(glob) = Glob::new(pattern) {
                if glob.compile_matcher().is_match(&action.0) {
                    ignored = true;
                    break;
                }
            }
        } else if action.0.contains(pattern) {
            ignored = true;
            break;
        }
    }

    if ignored {
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
        preceding_comments: None,
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
    fn test_find_tasks_circleci_orbs() {
        let yaml = r#"
version: 2.1
orbs:
  node: circleci/node@5.0.0
  slack: circleci/slack@4.1.0
"#;
        let (tree, content) = parse_yaml(yaml);
        let path = Path::new(".circleci/config.yml");
        let results = find_tasks(path, tree.root_node(), &content, &[]).unwrap();

        assert_eq!(results.len(), 2);

        let node_orb = results
            .iter()
            .find(|r| r.action.0 == "circleci/node")
            .unwrap();
        assert_eq!(node_orb.key, "orbs");
        assert_eq!(node_orb.current_tag.as_deref(), Some("5.0.0"));

        let slack_orb = results
            .iter()
            .find(|r| r.action.0 == "circleci/slack")
            .unwrap();
        assert_eq!(slack_orb.key, "orbs");
        assert_eq!(slack_orb.current_tag.as_deref(), Some("4.1.0"));
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

    #[test]
    fn test_find_tasks_multi_document() {
        let yaml = r#"
uses: actions/checkout@v1
---
uses: actions/setup-node@v2
"#;
        let (tree, content) = parse_yaml(yaml);
        let path = Path::new(".github/workflows/ci.yml");
        let results = find_tasks(path, tree.root_node(), &content, &[]).unwrap();

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].action.0, "actions/checkout");
        assert_eq!(results[1].action.0, "actions/setup-node");
    }

    #[test]
    fn test_find_tasks_malformed_yaml() {
        // Tree-sitter is resilient and should still find the dependency in partial/broken YAML
        let yaml = r#"
jobs:
  build:
    steps:
      - uses: actions/checkout@v3
    invalid_yaml_here: [
"#;
        let (tree, content) = parse_yaml(yaml);
        let path = Path::new(".github/workflows/ci.yml");
        let results = find_tasks(path, tree.root_node(), &content, &[]).unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].action.0, "actions/checkout");
    }

    #[test]
    fn test_find_tasks_empty_yaml() {
        let yaml = "";
        let (tree, content) = parse_yaml(yaml);
        let path = Path::new(".github/workflows/ci.yml");
        let results = find_tasks(path, tree.root_node(), &content, &[]).unwrap();

        assert!(results.is_empty());
    }

    #[test]
    fn test_find_tasks_complex_nesting() {
        let yaml = r#"
jobs:
  build:
    steps:
      - name: Checkout
        uses: actions/checkout@v3
      - name: Nested
        run: |
          echo "hello"
        env:
          IMAGE: "not-a-dependency"
      - image: redis:6.0 # This is a dependency
"#;
        let (tree, content) = parse_yaml(yaml);
        let path = Path::new(".github/workflows/ci.yml");
        let results = find_tasks(path, tree.root_node(), &content, &[]).unwrap();

        assert_eq!(results.len(), 2);
        assert!(results.iter().any(|r| r.action.0 == "actions/checkout"));
        assert!(results.iter().any(|r| r.action.0 == "redis"));
    }

    #[test]
    fn test_create_task_with_ports() {
        let yaml = r#"
- image: localhost:5000/my-image:v1.0.0
- image: localhost:5000/my-image
- image: docker://localhost:5000/my-image:v2.0.0
- image: docker://localhost:5000/my-image
"#;
        let (tree, content) = parse_yaml(yaml);
        let path = Path::new("bitbucket-pipelines.yml");
        let results = find_tasks(path, tree.root_node(), &content, &[]).unwrap();

        assert_eq!(results.len(), 4);

        assert_eq!(results[0].action.0, "localhost:5000/my-image");
        assert_eq!(results[0].current_tag.as_deref(), Some("v1.0.0"));

        assert_eq!(results[1].action.0, "localhost:5000/my-image");
        assert_eq!(results[1].current_tag.as_deref(), None);

        assert_eq!(results[2].action.0, "docker://localhost:5000/my-image");
        assert_eq!(results[2].current_tag.as_deref(), Some("v2.0.0"));

        assert_eq!(results[3].action.0, "docker://localhost:5000/my-image");
        assert_eq!(results[3].current_tag.as_deref(), None);
    }

    #[test]
    fn test_find_tasks_ignore_list_glob() {
        let yaml = r#"
- uses: actions/checkout@v3
- uses: actions/setup-node@v2
- uses: docker://gcr.io/internal/my-image@sha256:abc
- uses: docker://gcr.io/external/other-image@sha256:123
- uses: normal/action@v1
"#;
        let (tree, content) = parse_yaml(yaml);
        let path = Path::new(".github/workflows/ci.yml");

        // Ignore patterns:
        // 1. "actions/*" -> Matches actions/checkout and actions/setup-node
        // 2. "*internal/*" -> Matches docker://gcr.io/internal/my-image
        // 3. "normal/action" -> Matches normal/action (exact substring)
        let ignore_list = vec![
            "actions/*".to_string(),
            "*internal/*".to_string(),
            "normal/action".to_string(),
        ];

        let results = find_tasks(path, tree.root_node(), &content, &ignore_list).unwrap();

        // Only docker://gcr.io/external/other-image should remain!
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].action.0, "docker://gcr.io/external/other-image");
    }

    #[test]
    fn test_ci_provider_from_path_tekton_k8s() {
        assert_eq!(
            CiProvider::from_path(Path::new("tekton/task.yaml")),
            CiProvider::Tekton
        );
        assert_eq!(
            CiProvider::from_path(Path::new("k8s/deployment.yaml")),
            CiProvider::Kubernetes
        );
        assert_eq!(
            CiProvider::from_path(Path::new("pod.yaml")),
            CiProvider::Kubernetes
        );
    }

    #[test]
    fn test_find_tasks_tekton_bundle() {
        let yaml = r#"
apiVersion: tekton.dev/v1beta1
kind: Task
metadata:
  name: git-clone
spec:
  taskRef:
    name: git-clone
    bundle: gcr.io/tekton-releases/catalog/git-clone:0.1
"#;
        let (tree, content) = parse_yaml(yaml);
        let path = Path::new("tekton/git-clone.yaml");
        let results = find_tasks(path, tree.root_node(), &content, &[]).unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0].action.0,
            "gcr.io/tekton-releases/catalog/git-clone"
        );
        assert_eq!(results[0].current_tag.as_deref(), Some("0.1"));
        assert_eq!(results[0].key, "bundle");
    }

    #[test]
    fn test_find_tasks_kubernetes_image() {
        let yaml = r#"
apiVersion: v1
kind: Pod
metadata:
  name: web
spec:
  containers:
  - name: web
    image: nginx:1.21.0
"#;
        let (tree, content) = parse_yaml(yaml);
        let path = Path::new("pod.yaml");
        let results = find_tasks(path, tree.root_node(), &content, &[]).unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].action.0, "nginx");
        assert_eq!(results[0].current_tag.as_deref(), Some("1.21.0"));
        assert_eq!(results[0].key, "image");
    }

    #[test]
    fn test_find_tasks_preceding_comments() {
        let yaml = r#"
# This is a block comment
# that explains checkout.
- uses: actions/checkout@v3
"#;
        let (tree, content) = parse_yaml(yaml);
        let path = Path::new(".github/workflows/ci.yml");
        let results = find_tasks(path, tree.root_node(), &content, &[]).unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].action.0, "actions/checkout");
        assert_eq!(
            results[0].preceding_comments.as_deref(),
            Some("# This is a block comment\n# that explains checkout.")
        );
    }
}
