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
*   `verify_provenance`: Checks for a cosign signature manifest (`sha256-<hex>.sig`). `Ok(false)` means *unsigned*, which callers report as a warning, never as compromised.

---

## The Caching Decorator (`CachedProvider`)

To prevent API rate-limiting and accelerate execution, `CachedProvider<T: RemoteProvider>` decorates any remote provider with a two-tiered caching system:

1.  **Memory Cache**: Uses the `moka` crate for high-performance, asynchronous in-memory caching.
2.  **Disk Cache**: Uses the `cacache` crate for persistent, directory-based caching.
3.  **Offline Mode**: When offline mode is enabled, network requests are bypassed, and values are exclusively read from cache. If a cache miss occurs, `PinnerError::Offline` is returned.
4.  **Cache Keys**: Memory keys include the action, tag and YAML key. Disk keys are additionally scoped by a namespace (`CachedProvider::with_namespace`) that `lib.rs` builds from the configured provider URLs, so results cached for github.com are never served for a GitHub Enterprise host with the same repository names.

---

## Registry Resolution (`OciRegistryProvider`)

OCI container images are resolved to digests using standard registry APIs:
*   **Image Parsing** (`parse_image_ref`): Names without a registry host go to Docker Hub (`registry-1.docker.io`, with single-segment names under `library/`); `docker.io` and `index.docker.io` are normalized to it.
*   **Authentication**: Supports Docker credentials lookup via `docker-credential` helpers or explicit `--oci-username`/`--oci-password` (basic auth).
*   **Docker Hub Handling**: Pre-fetches a bearer token from `auth.docker.io`, saving a round-trip.
*   **Token Challenges**: Other registries (GHCR, Quay, GCR, …) are tried directly. On a `401` with `WWW-Authenticate: Bearer realm=…,service=…,scope=…`, a token is fetched from the realm (with credentials when available) and the request is retried. This is what makes anonymous pulls of public images work.
*   **Multi-Platform Digests**: The `Accept` header lists the OCI image index and Docker manifest list types first, so a tag resolves to the digest covering every platform rather than a single-platform fallback.
*   **HEAD First**: Manifests are requested with `HEAD` (free under Docker Hub pull limits), falling back to `GET` on `405`/`501` or when the digest header is missing. The returned `Docker-Content-Digest` must be a well-formed `sha256:` digest.
*   **Registry URL Template**: Formats requests using dynamic base URLs like `https://{registry}/v2/{repository}/manifests/{tag}`.

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
2.  **Pin vs. Upgrade**:
    *   `resolve_pin` hashes mutable references. Images without a tag are pinned as `latest`.
    *   `resolve_upgrade` picks a candidate with `select_candidate_tag` (`latest`: latest release; `major`/`minor`: highest tag within the current major or major.minor) and applies it only if `is_newer`. `commit` follows the default branch head.
    *   Images re-resolve the tag they track (`UpdateTask::logical_tag()`: version comment or inline `name:tag@digest` tag). A digest-pinned image with no recoverable tag is left alone.
    *   `get_upgrade_candidate` (used by `scan`) shares the same selection without the newer-than check.
3.  **Asynchronous Stream Concurrency**:
    *   Resolved groups are converted into a future stream.
    *   Uses `futures::stream::StreamExt::buffer_unordered(concurrency)` to process tasks concurrently up to the user-defined limits, preventing connection exhaustion.
    *   Propagates critical errors (e.g., OAuth rate limits) immediately while isolating non-fatal errors (e.g., a single invalid custom action) so that other tasks continue processing.

---

## Vulnerability & Security Auditing (`OsvClient`)

In addition to resolving references, `pinner` integrates with security auditing databases:
- **OSV Vulnerability Check**: The `OsvClient` (`src/resolver/osv.rs`) queries the OpenSSF OSV (Open Source Vulnerability) database to check if a resolved commit SHA contains known vulnerabilities or has been flagged as compromised.
- **Vulnerability Checks during Verification & Scanning**: Used by the `verify` command (with `--check-osv`, also available via GitHub Action `check-osv: true`) and the `scan` command to flag and blacklist compromised hashes. `scan` treats advisories mentioning malicious/backdoor/hijacked code as compromised and others as ordinary vulnerabilities. A failed lookup is reported as a warning and the reference is neither vetted nor blacklisted.


