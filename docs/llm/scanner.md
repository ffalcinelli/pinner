# Pinner: Scanner & AST Parser Layer

The Scanner layer locates and parses CI/CD configurations to extract pinning targets. It operates in two phases: filesystem traversal (`walker.rs`) and syntax-tree parsing (`parser.rs`).

---

## Filesystem Traversal (`walker.rs`)

The scanner recursively walks target paths looking for workflow configurations. It has the following characteristics:

1.  **Ignore Patterns**: Uses the `ignore` crate (specifically `WalkBuilder` and `OverrideBuilder`) to respect `.gitignore` rules and process custom ignore lists configured in `.pinner.toml` or CLI args.
2.  **YAML File Filtering**: Filters files matching `.yml` or `.yaml` extensions.
3.  **Parallel Parsing via Rayon**:
    *   To keep the async tokio runtime responsive, directory walking and parsing run inside a `tokio::task::spawn_blocking` pool.
    *   Paths are parsed concurrently using `Rayon` (`into_par_iter`).
    *   **Thread-Local Parsers**: Tree-Sitter parsers are not thread-safe. To prevent expensive instantiation and lock contention, a thread-local parser (`static PARSER: RefCell<TSParser>`) is maintained and reset for each file.

---

## AST Parsing using Tree-Sitter (`parser.rs`)

`pinner` uses `tree-sitter-yaml` to build concrete syntax trees of workflow files. This is far more robust than regex-based parsing because it respects quotes, indentation, and structure.

### Tree-Sitter AST Query

The `USES_QUERY` static query captures:
1.  Standard key-value pairs (`block_mapping_pair`) where the key matches any of the dependency triggers (`uses`, `image`, `pipe`, `include`, `ref`, `task`, `template`).
2.  CircleCI nested `orbs` declarations (nested map parser).
3.  Line comments (`comment`), which are matched with preceding values to preserve comments.

```query
; Capture standard key-value pairs where the key matches our known dependency triggers.
(block_mapping_pair
  key: [
    (flow_node (plain_scalar (string_scalar) @key))
    (plain_scalar (string_scalar) @key)
  ]
  value: (_) @value
  (#match? @key "^(uses|pipe|image|include|ref|task|template)$"))

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
```

---

## Provider-Specific Parsing Rules

The system determines the CI provider via the file path (e.g., `.github/workflows` matches `CiProvider::GitHub`). Each provider activates parsing rules for specific YAML keys:

| CI Provider | Match Path Pattern | Supported Keys | Resolution Target Type |
| :--- | :--- | :--- | :--- |
| **GitHub** | `.github/workflows` | `uses`, `image` | GitHub Action or OCI Image |
| **GitLab** | `.gitlab-ci` | `include`, `image`, `ref` | GitLab Repo, Template, or OCI Image |
| **Bitbucket** | `bitbucket-pipelines` | `pipe`, `image` | Bitbucket Pipe or OCI Image |
| **CircleCI** | `.circleci` | `image`, `orbs` | CircleCI Orb or OCI Image |
| **Azure DevOps**| `azure-pipelines` | `task`, `template`, `image` | Azure Task/Template or OCI Image |
| **Forgejo / Gitea**| `.forgejo/workflows`, `.gitea/workflows` | `uses`, `image` | Forgejo Action or OCI Image |
| **AWS CodeBuild**| `buildspec` | `image` | OCI Image |

---

## GitLab Special Case: Virtual Dependencies

In GitLab CI, references to external templates are written with separate `project` and `ref` keys in a YAML map:

```yaml
include:
  - project: 'my-group/my-project'
    ref: 'v1.0.0'
    file: '/templates/ci.yml'
```

When parsing the `ref` key, the parser looks up its sibling nodes in the AST to fetch the `project` key value. It combines them into a virtual dependency format: `my-group/my-project@v1.0.0` with `ref` as the parsing key. This allows the resolver to treat it as a standard repository action.

---

## Trailing Comments Association

To distinguish between a code comment on the same line and comments on other lines, `parser.rs` uses a buffering heuristic:
1.  When a value capture is processed, its start line and position are stored in `last_value`.
2.  If the next capture is a `(comment)` node and it resides on the **same line** (i.e. `comment.start_position().row == value.start_position().row`), it is captured as `UpdateTask::comment`.
3.  If the comment is on a different line, the buffered value is pushed without a comment, and the comment is discarded (since it does not annotate this dependency).
