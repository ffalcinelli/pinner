# Pinner: Resolver & Provider Layer

The Resolver layer translates symbolic tags (e.g., `@v3`, `:latest`) into immutable references (SHA-1 commit hashes or OCI image digests) using network clients and local caching.

---

## Core Traits

The Resolver is highly modular and utilizes dependency injection via two main traits, enabling offline stubbing and mock-based testing:

### 1. `RemoteProvider` (`src/resolver/provider.rs`)
Used for action/template repository resolution (e.g., GitHub, GitLab):
*   `get_commit_sha`: Retrieves the commit SHA-1 for a tag or branch.
*   `get_latest_release`: Discovers the latest official release version tag.
*   `list_tags`: Lists all tags associated with the repository.
*   `get_default_branch`: Identifies the primary branch name (e.g., `main` or `master`).

### 2. `RegistryProvider` (`src/resolver/registry.rs`)
Used for OCI container image resolution:
*   `resolve_digest`: Maps an image tag (e.g., `ubuntu:latest`) to its SHA-256 digest (`sha256:abc...`).
*   `verify_provenance`: Inspects and verifies cryptographic signatures or provenance manifests.

---

## The Caching Decorator (`CachedProvider`)

To prevent API rate-limiting and accelerate execution, `CachedProvider<T: RemoteProvider>` decorates any remote provider with a two-tiered caching system:

1.  **Memory Cache**: Uses the `moka` crate for high-performance, asynchronous in-memory caching.
2.  **Disk Cache**: Uses the `cacache` crate for persistent, directory-based caching.
3.  **Offline Mode**: When offline mode is enabled, network requests are bypassed, and values are exclusively read from cache. If a cache miss occurs, `PinnerError::Offline` is returned.

---

## Registry Resolution (`OciRegistryProvider`)

OCI container images are resolved to digests using standard registry APIs:
*   **Authentication**: Supports Docker credentials lookup via `docker-credential` helpers or fallbacks to environment credentials (basic auth).
*   **Docker Hub Handling**: Automatically obtains bearer tokens from `auth.docker.io` for repository pulling.
*   **Registry URL Template**: Formats requests using dynamic base URLs like `https://{registry}/v2/{repository}/manifests/{tag}` and accepts OCI-standard media headers.

---

## Provider Registry & Routing Logic

The `ProviderRegistry` holds the collection of remote providers. When the `Resolver` receives a dependency, it routes it using a specific precedence rule:

```
                  ┌──────────────────────────────┐
                  │   Is there an explicit       │
                  │   domain name match?         │
                  └──────────────┬───────────────┘
                                 │
                    Yes ┌────────┴────────┐ No
         ┌──────────────▼──────┐   ┌──────▼──────────────────────┐
         │ Route to matching   │   │ Is there a unique YAML key  │
         │ domain provider.    │   │ match (e.g., pipe, orbs)?   │
         │ (e.g., gitlab.com)  │   └──────────────┬──────────────┘
         └─────────────────────┘                  │
                                     Yes ┌────────┴────────┐ No
                          ┌──────────────▼──────┐   ┌──────▼───────────────┐
                          │ Route to matching   │   │ Default to:          │
                          │ key provider.       │   │ GitHub Provider      │
                          └─────────────────────┘   └──────────────────────┘
```

### Registered Providers:
1.  **GitHub** (`ReqwestGithubProvider`): Handles `github.com` references for `uses` and `image` keys.
2.  **Azure** (`ReqwestAzureProvider`): Wraps the GitHub provider because Azure pipeline tasks are typically fetched from GitHub.
3.  **Bitbucket** (`ReqwestBitbucketProvider`): Handles `bitbucket.org` references and the `pipe` key.
4.  **GitLab** (`ReqwestGitLabProvider`): Handles `gitlab.com` references and the `include`/`ref` keys.
5.  **Forgejo** (`ReqwestForgejoProvider`): Handles `codeberg.org`/Forgejo self-hosted repositories.
6.  **CircleCI** (`ReqwestCircleCiProvider`): Handles CircleCI `orbs`.

---

## Batch Coalescing & Concurrency (`Resolver`)

The high-level resolution engine is implemented in `Resolver` (`src/resolver/unified.rs`):

1.  **Grouping**: Before making any API requests, the resolver groups incoming `UpdateTask`s by `(action, current_tag, key)`. For example, if `actions/checkout@v3` is referenced 15 times, the resolver resolves it exactly once, eliminating duplicate requests.
2.  **Asynchronous Stream Concurrency**:
    *   Resolved groups are converted into a future stream.
    *   Uses `futures::stream::StreamExt::buffer_unordered(concurrency)` to process tasks concurrently up to the user-defined limits, preventing connection exhaustion.
    *   Propagates critical errors (e.g., OAuth rate limits) immediately while isolating non-fatal errors (e.g., a single invalid custom action) so that other tasks continue processing.
