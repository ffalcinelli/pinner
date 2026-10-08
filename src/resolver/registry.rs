use crate::core::is_oci_digest;
use crate::error::PinnerError;
use async_trait::async_trait;
use reqwest::header::{
    HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, USER_AGENT, WWW_AUTHENTICATE,
};
use reqwest::{Method, StatusCode};
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use reqwest_retry::{policies::ExponentialBackoff, RetryTransientMiddleware};
use serde::Deserialize;
use std::collections::HashMap;

#[cfg(test)]
use mockall::automock;

#[cfg_attr(test, automock)]
#[async_trait]
pub trait RegistryProvider: Send + Sync {
    /// Resolves a docker image tag to its digest.
    async fn resolve_digest(&self, image: &str, tag: &str) -> Result<String, PinnerError>;

    /// Verifies provenance/signature of a docker image.
    async fn verify_provenance(&self, image: &str, digest: &str) -> Result<bool, PinnerError>;
}

/// Media types accepted when fetching a manifest.
///
/// Multi-platform index types come first so a tag always resolves to the digest that
/// covers every platform, never to the single-platform (usually linux/amd64) manifest
/// that spec-compliant registries fall back to when an index is not accepted.
const MANIFEST_ACCEPT: &str = "application/vnd.oci.image.index.v1+json, \
application/vnd.docker.distribution.manifest.list.v2+json, \
application/vnd.oci.image.manifest.v1+json, \
application/vnd.docker.distribution.manifest.v2+json";

/// Host serving the Docker Hub registry API.
const DOCKER_HUB: &str = "registry-1.docker.io";

/// Splits an image name into its registry host and repository path.
///
/// Images without an explicit registry live on Docker Hub, where single-segment
/// names are official images under `library/` (e.g. `alpine` is `library/alpine`).
pub(crate) fn parse_image_ref(image: &str) -> (&str, String) {
    let (registry, repository) = match image.split_once('/') {
        Some((first, rest))
            if first.contains('.') || first.contains(':') || first == "localhost" =>
        {
            match first {
                "docker.io" | "index.docker.io" | DOCKER_HUB => (DOCKER_HUB, rest),
                _ => (first, rest),
            }
        }
        _ => (DOCKER_HUB, image),
    };

    if registry == DOCKER_HUB && !repository.contains('/') {
        (registry, format!("library/{}", repository))
    } else {
        (registry, repository.to_string())
    }
}

/// Parses the parameters of a `WWW-Authenticate: Bearer realm="…",service="…",scope="…"`
/// challenge. Returns `None` for non-Bearer challenges or when no realm is given.
fn parse_bearer_challenge(header: &str) -> Option<HashMap<String, String>> {
    let (scheme, mut rest) = header.trim().split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }

    let mut params = HashMap::new();
    while !rest.trim().is_empty() {
        let (key, after) = rest.split_once('=')?;
        let after = after.trim_start();
        let (value, remainder) = match after.strip_prefix('"') {
            Some(quoted) => {
                let end = quoted.find('"')?;
                (&quoted[..end], &quoted[end + 1..])
            }
            None => after.split_at(after.find(',').unwrap_or(after.len())),
        };
        params.insert(key.trim().to_ascii_lowercase(), value.trim().to_string());
        rest = remainder.trim_start().trim_start_matches(',');
    }

    params.contains_key("realm").then_some(params)
}

fn build_client() -> ClientWithMiddleware {
    let mut h = HeaderMap::new();
    h.insert(USER_AGENT, HeaderValue::from_static("pinner"));

    let reqwest_client = reqwest::Client::builder()
        .default_headers(h)
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());

    let retry_policy = ExponentialBackoff::builder().build_with_max_retries(3);
    ClientBuilder::new(reqwest_client)
        .with(RetryTransientMiddleware::new_with_policy(retry_policy))
        .build()
}

/// Implementation of [`RegistryProvider`] for OCI-compliant registries.
#[derive(Clone)]
pub struct OciRegistryProvider {
    client: ClientWithMiddleware,
    auth_url: String,
    base_url_template: String,
    username: Option<String>,
    password: Option<String>,
    offline: bool,
}

impl Default for OciRegistryProvider {
    fn default() -> Self {
        Self::new(None, None)
    }
}

impl OciRegistryProvider {
    pub fn new(username: Option<String>, password: Option<String>) -> Self {
        Self {
            client: build_client(),
            auth_url: "https://auth.docker.io/token".to_string(),
            base_url_template: "https://{registry}/v2/{repository}/manifests/{tag}".to_string(),
            username,
            password,
            offline: false,
        }
    }

    /// Set offline mode.
    pub fn with_offline(mut self, offline: bool) -> Self {
        self.offline = offline;
        self
    }

    #[cfg(test)]
    pub fn with_base_urls(auth_url: String, base_url_template: String) -> Self {
        Self {
            auth_url,
            base_url_template,
            ..Self::new(None, None)
        }
    }

    /// Fetches an anonymous or authenticated pull token for Docker Hub up front,
    /// saving a round-trip through the 401 challenge. Other registries return an
    /// empty token and are authenticated on demand (see [`Self::fetch_manifest`]).
    async fn get_token(&self, registry: &str, repository: &str) -> Result<String, PinnerError> {
        if registry != DOCKER_HUB {
            return Ok(String::new());
        }
        let url = format!(
            "{}?service=registry.docker.io&scope=repository:{}:pull",
            self.auth_url, repository
        );
        self.request_token(&url, registry).await
    }

    /// Exchanges a Bearer challenge for a token, as described by the OCI distribution
    /// spec. Credentials for `registry` are sent when available.
    async fn token_from_challenge(
        &self,
        challenge: &HashMap<String, String>,
        registry: &str,
        repository: &str,
    ) -> Result<String, PinnerError> {
        let default_scope = format!("repository:{}:pull", repository);
        let mut query = vec![(
            "scope",
            challenge
                .get("scope")
                .map_or(default_scope.as_str(), String::as_str),
        )];
        if let Some(service) = challenge.get("service") {
            query.push(("service", service));
        }
        let url = reqwest::Url::parse_with_params(&challenge["realm"], &query).map_err(|e| {
            PinnerError::Api(format!("Invalid auth realm from {}: {}", registry, e))
        })?;
        self.request_token(url.as_str(), registry).await
    }

    async fn request_token(&self, url: &str, registry: &str) -> Result<String, PinnerError> {
        let mut rb = self.client.get(url);
        if let (Some(u), Some(p)) = self.get_credentials(registry) {
            rb = rb.basic_auth(u, Some(p));
        }

        let resp = rb
            .send()
            .await
            .map_err(|e| PinnerError::Api(format!("Failed to send auth request: {}", e)))?;

        if !resp.status().is_success() {
            return Err(PinnerError::Api(format!(
                "Failed to authenticate with {}: {}",
                registry,
                resp.status()
            )));
        }

        #[derive(Deserialize)]
        struct TokenResponse {
            token: Option<String>,
            access_token: Option<String>,
        }

        let res: TokenResponse = resp
            .json()
            .await
            .map_err(|e| PinnerError::Api(e.to_string()))?;
        res.token.or(res.access_token).ok_or_else(|| {
            PinnerError::Api(format!(
                "Auth response from {} contained no token",
                registry
            ))
        })
    }

    fn get_credentials(&self, registry: &str) -> (Option<String>, Option<String>) {
        if let (Some(u), Some(p)) = (&self.username, &self.password) {
            return (Some(u.clone()), Some(p.clone()));
        }

        let lookup_registry = if registry == DOCKER_HUB || registry == "docker.io" {
            "https://index.docker.io/v1/"
        } else {
            registry
        };

        // Try to get from docker config
        #[cfg(not(test))]
        {
            use docker_credential::{get_credential, DockerCredential};
            match get_credential(lookup_registry) {
                Ok(DockerCredential::UsernamePassword(username, password)) => {
                    (Some(username), Some(password))
                }
                _ => {
                    if lookup_registry != registry {
                        if let Ok(DockerCredential::UsernamePassword(username, password)) =
                            get_credential(registry)
                        {
                            return (Some(username), Some(password));
                        }
                    }
                    (None, None)
                }
            }
        }
        #[cfg(test)]
        {
            let _ = (registry, lookup_registry);
            (None, None)
        }
    }

    async fn send_manifest_request(
        &self,
        method: Method,
        url: &str,
        authorization: Option<&str>,
    ) -> Result<reqwest::Response, PinnerError> {
        let mut rb = self
            .client
            .request(method, url)
            .header(ACCEPT, MANIFEST_ACCEPT);
        if let Some(auth) = authorization {
            rb = rb.header(AUTHORIZATION, auth);
        }
        rb.send()
            .await
            .map_err(|e| PinnerError::Api(format!("Failed to fetch manifest {}: {}", url, e)))
    }

    /// Requests a manifest, authenticating as needed.
    ///
    /// Docker Hub is pre-authenticated; other registries are tried with Basic
    /// credentials (if configured) and, on a `401` Bearer challenge, retried with a
    /// token obtained from the challenge realm. This is what makes anonymous pulls
    /// from GHCR, Quay, GCR and similar registries work.
    async fn fetch_manifest(
        &self,
        method: Method,
        registry: &str,
        repository: &str,
        reference: &str,
    ) -> Result<reqwest::Response, PinnerError> {
        let url = self
            .base_url_template
            .replace("{registry}", registry)
            .replace("{repository}", repository)
            .replace("{tag}", reference);

        let token = self.get_token(registry, repository).await?;
        let authorization = if !token.is_empty() {
            Some(format!("Bearer {}", token))
        } else if let (Some(u), Some(p)) = self.get_credentials(registry) {
            Some(format!("Basic {}", b64_encode(&format!("{}:{}", u, p))))
        } else {
            None
        };

        let resp = self
            .send_manifest_request(method.clone(), &url, authorization.as_deref())
            .await?;
        if resp.status() != StatusCode::UNAUTHORIZED {
            return Ok(resp);
        }

        let challenge = resp
            .headers()
            .get(WWW_AUTHENTICATE)
            .and_then(|h| h.to_str().ok())
            .and_then(parse_bearer_challenge);
        match challenge {
            Some(challenge) => {
                let token = self
                    .token_from_challenge(&challenge, registry, repository)
                    .await?;
                self.send_manifest_request(method, &url, Some(&format!("Bearer {}", token)))
                    .await
            }
            None => Ok(resp),
        }
    }

    /// Like [`Self::fetch_manifest`], but uses `HEAD` (which does not count against
    /// Docker Hub pull limits) and falls back to `GET` for registries that reject it.
    async fn head_manifest(
        &self,
        registry: &str,
        repository: &str,
        reference: &str,
    ) -> Result<reqwest::Response, PinnerError> {
        let resp = self
            .fetch_manifest(Method::HEAD, registry, repository, reference)
            .await?;
        if matches!(
            resp.status(),
            StatusCode::METHOD_NOT_ALLOWED | StatusCode::NOT_IMPLEMENTED
        ) {
            return self
                .fetch_manifest(Method::GET, registry, repository, reference)
                .await;
        }
        Ok(resp)
    }

    async fn verify_signature(&self, image: &str, digest: &str) -> Result<bool, PinnerError> {
        if self.offline {
            return Err(PinnerError::Offline(format!(
                "Network request to verify OCI signature for {}@{} is disabled in offline mode",
                image, digest
            )));
        }

        let digest_str = if digest.starts_with("sha256:") {
            digest.to_string()
        } else {
            self.resolve_digest(image, digest).await?
        };

        let (registry, repository) = parse_image_ref(image);
        let digest_hex = digest_str.strip_prefix("sha256:").unwrap_or(&digest_str);
        let sig_tag = format!("sha256-{}.sig", digest_hex);

        let resp = self.head_manifest(registry, &repository, &sig_tag).await?;
        if resp.status().is_success() {
            Ok(true)
        } else if resp.status() == StatusCode::NOT_FOUND {
            Ok(false)
        } else {
            Err(PinnerError::Api(format!(
                "Failed to fetch signature manifest for {}: HTTP {}",
                image,
                resp.status()
            )))
        }
    }
}

fn content_digest(resp: &reqwest::Response) -> Option<String> {
    resp.headers()
        .get("Docker-Content-Digest")
        .and_then(|h| h.to_str().ok())
        .map(|s| s.trim().to_string())
}

#[async_trait]
impl RegistryProvider for OciRegistryProvider {
    async fn resolve_digest(&self, image: &str, tag: &str) -> Result<String, PinnerError> {
        if self.offline {
            return Err(PinnerError::Offline(format!(
                "Network request to resolve OCI digest for {}@{} is disabled in offline mode",
                image, tag
            )));
        }

        let (registry, repository) = parse_image_ref(image);
        let mut resp = self.head_manifest(registry, &repository, tag).await?;

        // A few registries omit the digest header on HEAD responses.
        if resp.status().is_success() && content_digest(&resp).is_none() {
            resp = self
                .fetch_manifest(Method::GET, registry, &repository, tag)
                .await?;
        }

        if !resp.status().is_success() {
            return Err(PinnerError::Api(format!(
                "Failed to fetch manifest for {}:{}: HTTP {}",
                image,
                tag,
                resp.status()
            )));
        }

        let digest = content_digest(&resp).ok_or_else(|| {
            PinnerError::Api(format!(
                "Digest not found in registry response for {}:{}",
                image, tag
            ))
        })?;
        if !is_oci_digest(&digest) {
            return Err(PinnerError::Api(format!(
                "Registry returned an invalid digest '{}' for {}:{}",
                digest, image, tag
            )));
        }
        Ok(digest)
    }

    async fn verify_provenance(&self, image: &str, digest: &str) -> Result<bool, PinnerError> {
        if self.offline {
            return Err(PinnerError::Offline(format!(
                "Network request to verify OCI provenance for {}@{} is disabled in offline mode",
                image, digest
            )));
        }
        self.verify_signature(image, digest).await
    }
}

fn b64_encode(s: &str) -> String {
    use base64::{engine::general_purpose, Engine as _};
    general_purpose::STANDARD.encode(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::Server;

    #[test]
    fn test_b64_encode() {
        assert_eq!(b64_encode(""), "");
        assert_eq!(b64_encode("user:pass"), "dXNlcjpwYXNz");
        assert_eq!(b64_encode("hello world"), "aGVsbG8gd29ybGQ=");
    }

    #[tokio::test]
    async fn test_resolve_digest_docker_hub() {
        let mut server = Server::new_async().await;
        let token_resp = r#"{"token":"test-token"}"#;
        let auth_path = "/token";
        let _m1 = server
            .mock("GET", mockito::Matcher::Any)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(token_resp)
            .create_async()
            .await;

        let digest = "sha256:1111111111111111111111111111111111111111111111111111111111111111";
        let _m2 = server
            .mock("HEAD", "/v2/library/alpine/manifests/latest")
            .with_status(200)
            .with_header("Docker-Content-Digest", digest)
            .create_async()
            .await;

        let mut provider = OciRegistryProvider::new(None, None);
        provider.auth_url = format!("{}{}", server.url(), auth_path);
        provider.base_url_template =
            format!("{}{}", server.url(), "/v2/{repository}/manifests/{tag}");

        let res = provider.resolve_digest("alpine", "latest").await.unwrap();
        assert_eq!(res, digest);
    }

    #[tokio::test]
    async fn test_resolve_digest_ghcr() {
        let mut server = Server::new_async().await;
        let digest = "sha256:2222222222222222222222222222222222222222222222222222222222222222";
        let _m = server
            .mock("HEAD", "/v2/my-org/my-repo/manifests/v1")
            .with_status(200)
            .with_header("Docker-Content-Digest", digest)
            .create_async()
            .await;

        let mut provider = OciRegistryProvider::new(None, None);
        provider.base_url_template =
            format!("{}{}", server.url(), "/v2/{repository}/manifests/{tag}");

        let res = provider
            .resolve_digest("ghcr.io/my-org/my-repo", "v1")
            .await
            .unwrap();
        assert_eq!(res, digest);
    }

    #[tokio::test]
    async fn test_resolve_digest_invalid_token_json() {
        let mut server = Server::new_async().await;
        let _m = server
            .mock("GET", mockito::Matcher::Any)
            .with_status(200)
            .with_body("invalid json")
            .create_async()
            .await;

        let mut provider = OciRegistryProvider::new(None, None);
        provider.auth_url = server.url();

        let res = provider.resolve_digest("alpine", "latest").await;
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn test_oci_auth_headers() {
        let mut server = Server::new_async().await;
        let _m = server
            .mock("HEAD", "/v2/repo/manifests/latest")
            .with_status(200)
            .with_header(
                "Docker-Content-Digest",
                "sha256:1111111111111111111111111111111111111111111111111111111111111111",
            )
            .create_async()
            .await;

        let mut provider = OciRegistryProvider::new(None, None);
        provider.base_url_template =
            format!("{}{}", server.url(), "/v2/{repository}/manifests/{tag}");

        provider
            .resolve_digest("localhost/repo", "latest")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_oci_auth_with_credentials() {
        let mut server = Server::new_async().await;
        let auth = b64_encode("user:pass");
        let _m = server
            .mock("HEAD", "/v2/repo/manifests/latest")
            .match_header("Authorization", format!("Basic {}", auth).as_str())
            .with_status(200)
            .with_header(
                "Docker-Content-Digest",
                "sha256:1111111111111111111111111111111111111111111111111111111111111111",
            )
            .create_async()
            .await;

        let mut provider = OciRegistryProvider::new(Some("user".into()), Some("pass".into()));
        provider.base_url_template =
            format!("{}{}", server.url(), "/v2/{repository}/manifests/{tag}");

        provider
            .resolve_digest("localhost/repo", "latest")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_oci_auth_ecr_fallback() {
        let mut server = Server::new_async().await;
        let ecr_registry = "123456789012.dkr.ecr.us-east-1.amazonaws.com";
        let _m = server
            .mock("HEAD", "/v2/my-repo/manifests/latest")
            .with_status(200)
            .with_header(
                "Docker-Content-Digest",
                "sha256:2222222222222222222222222222222222222222222222222222222222222222",
            )
            .create_async()
            .await;

        let mut provider = OciRegistryProvider::new(None, None);
        provider.base_url_template =
            format!("{}{}", server.url(), "/v2/{repository}/manifests/{tag}");

        let res = provider
            .resolve_digest(&format!("{}/my-repo", ecr_registry), "latest")
            .await
            .unwrap();
        assert_eq!(
            res,
            "sha256:2222222222222222222222222222222222222222222222222222222222222222"
        );
    }

    #[tokio::test]
    async fn test_oci_registry_provider_offline_mode() {
        let provider = OciRegistryProvider::new(None, None).with_offline(true);
        let res = provider.resolve_digest("alpine", "latest").await;
        assert!(res.is_err());
        assert!(matches!(res.unwrap_err(), PinnerError::Offline(_)));

        let res = provider.verify_provenance("alpine", "sha256:digest").await;
        assert!(res.is_err());
        assert!(matches!(res.unwrap_err(), PinnerError::Offline(_)));
    }

    #[test]
    fn test_oci_registry_provider_default() {
        let _ = OciRegistryProvider::default();
    }

    #[test]
    fn test_oci_registry_provider_with_base_urls() {
        let _ = OciRegistryProvider::with_base_urls("auth_url".to_string(), "base_url".to_string());
    }

    #[tokio::test]
    async fn test_oci_registry_provider_verify_provenance_mocked() {
        let mut server = Server::new_async().await;
        let _m = server
            .mock("HEAD", "/v2/repo/manifests/sha256-1111111111111111111111111111111111111111111111111111111111111111.sig")
            .with_status(200)
            .create_async()
            .await;

        let mut provider = OciRegistryProvider::new(None, None);
        provider.base_url_template =
            format!("{}{}", server.url(), "/v2/{repository}/manifests/{tag}");

        let res = provider
            .verify_provenance(
                "localhost/repo",
                "sha256:1111111111111111111111111111111111111111111111111111111111111111",
            )
            .await
            .unwrap();
        assert!(res);
    }

    #[tokio::test]
    async fn test_oci_registry_provider_verify_provenance_not_found() {
        let mut server = Server::new_async().await;
        let _m = server
            .mock("HEAD", "/v2/repo/manifests/sha256-1111111111111111111111111111111111111111111111111111111111111111.sig")
            .with_status(404)
            .create_async()
            .await;

        let mut provider = OciRegistryProvider::new(None, None);
        provider.base_url_template =
            format!("{}{}", server.url(), "/v2/{repository}/manifests/{tag}");

        let res = provider
            .verify_provenance(
                "localhost/repo",
                "sha256:1111111111111111111111111111111111111111111111111111111111111111",
            )
            .await
            .unwrap();
        assert!(!res);
    }

    #[tokio::test]
    async fn test_oci_registry_provider_verify_provenance_tag_mocked() {
        let mut server = Server::new_async().await;
        let _m1 = server
            .mock("HEAD", "/v2/repo/manifests/latest")
            .with_status(200)
            .with_header(
                "Docker-Content-Digest",
                "sha256:1111111111111111111111111111111111111111111111111111111111111111",
            )
            .create_async()
            .await;
        let _m2 = server
            .mock("HEAD", "/v2/repo/manifests/sha256-1111111111111111111111111111111111111111111111111111111111111111.sig")
            .with_status(200)
            .create_async()
            .await;

        let mut provider = OciRegistryProvider::new(None, None);
        provider.base_url_template =
            format!("{}{}", server.url(), "/v2/{repository}/manifests/{tag}");

        let res = provider
            .verify_provenance("localhost/repo", "latest")
            .await
            .unwrap();
        assert!(res);
    }

    #[tokio::test]
    async fn test_resolve_digest_docker_hub_with_credentials() {
        let mut server = Server::new_async().await;
        let token_resp = r#"{"token":"test-token"}"#;
        let auth_path = "/token";
        let auth = b64_encode("user:pass");
        let _m1 = server
            .mock("GET", mockito::Matcher::Any)
            .match_header("Authorization", format!("Basic {}", auth).as_str())
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(token_resp)
            .create_async()
            .await;

        let digest = "sha256:1111111111111111111111111111111111111111111111111111111111111111";
        let _m2 = server
            .mock("HEAD", "/v2/library/alpine/manifests/latest")
            .with_status(200)
            .with_header("Docker-Content-Digest", digest)
            .create_async()
            .await;

        let mut provider = OciRegistryProvider::new(Some("user".into()), Some("pass".into()));
        provider.auth_url = format!("{}{}", server.url(), auth_path);
        provider.base_url_template =
            format!("{}{}", server.url(), "/v2/{repository}/manifests/{tag}");

        let res = provider.resolve_digest("alpine", "latest").await.unwrap();
        assert_eq!(res, digest);
    }

    #[tokio::test]
    async fn test_resolve_digest_docker_hub_auth_failure() {
        let mut server = Server::new_async().await;
        let auth_path = "/token";
        let _m1 = server
            .mock("GET", mockito::Matcher::Any)
            .with_status(401)
            .create_async()
            .await;

        let mut provider = OciRegistryProvider::new(None, None);
        provider.auth_url = format!("{}{}", server.url(), auth_path);

        let res = provider.resolve_digest("alpine", "latest").await;
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn test_resolve_digest_manifest_failure() {
        let mut server = Server::new_async().await;
        let _m = server
            .mock("HEAD", "/v2/repo/manifests/latest")
            .with_status(404)
            .create_async()
            .await;

        let mut provider = OciRegistryProvider::new(None, None);
        provider.base_url_template =
            format!("{}{}", server.url(), "/v2/{repository}/manifests/{tag}");

        let res = provider.resolve_digest("localhost/repo", "latest").await;
        assert!(res.is_err());
    }

    const DIGEST: &str = "sha256:3333333333333333333333333333333333333333333333333333333333333333";

    fn provider_for(server: &Server) -> OciRegistryProvider {
        OciRegistryProvider::with_base_urls(
            format!("{}/token", server.url()),
            format!("{}{}", server.url(), "/v2/{repository}/manifests/{tag}"),
        )
    }

    #[test]
    fn test_parse_image_ref() {
        assert_eq!(
            parse_image_ref("alpine"),
            (DOCKER_HUB, "library/alpine".to_string())
        );
        assert_eq!(
            parse_image_ref("cimg/base"),
            (DOCKER_HUB, "cimg/base".to_string())
        );
        assert_eq!(
            parse_image_ref("docker.io/alpine"),
            (DOCKER_HUB, "library/alpine".to_string())
        );
        assert_eq!(
            parse_image_ref("docker.io/library/node"),
            (DOCKER_HUB, "library/node".to_string())
        );
        assert_eq!(
            parse_image_ref("ghcr.io/org/repo"),
            ("ghcr.io", "org/repo".to_string())
        );
        assert_eq!(
            parse_image_ref("localhost:5000/repo"),
            ("localhost:5000", "repo".to_string())
        );
    }

    #[test]
    fn test_parse_bearer_challenge() {
        let c = parse_bearer_challenge(
            r#"Bearer realm="https://ghcr.io/token",service="ghcr.io",scope="repository:a/b:pull,push""#,
        )
        .unwrap();
        assert_eq!(c["realm"], "https://ghcr.io/token");
        assert_eq!(c["service"], "ghcr.io");
        assert_eq!(c["scope"], "repository:a/b:pull,push");

        let c = parse_bearer_challenge("bearer realm=https://r/token, service=svc").unwrap();
        assert_eq!(c["realm"], "https://r/token");
        assert_eq!(c["service"], "svc");

        assert!(parse_bearer_challenge(r#"Basic realm="x""#).is_none());
        assert!(parse_bearer_challenge(r#"Bearer service="x""#).is_none());
    }

    #[tokio::test]
    async fn test_resolve_digest_requests_multi_arch_index() {
        let mut server = Server::new_async().await;
        let _m = server
            .mock("HEAD", "/v2/org/repo/manifests/v1")
            .match_header(
                "Accept",
                mockito::Matcher::Regex(
                    r"^application/vnd\.oci\.image\.index\.v1\+json, application/vnd\.docker\.distribution\.manifest\.list\.v2\+json".into(),
                ),
            )
            .with_status(200)
            .with_header("Docker-Content-Digest", DIGEST)
            .create_async()
            .await;

        let res = provider_for(&server)
            .resolve_digest("ghcr.io/org/repo", "v1")
            .await
            .unwrap();
        assert_eq!(res, DIGEST);
    }

    #[tokio::test]
    async fn test_resolve_digest_follows_bearer_challenge() {
        let mut server = Server::new_async().await;
        let challenge = format!(
            r#"Bearer realm="{}/token",service="ghcr.io",scope="repository:org/repo:pull""#,
            server.url()
        );
        let _unauth = server
            .mock("HEAD", "/v2/org/repo/manifests/v1")
            .match_header("Authorization", mockito::Matcher::Missing)
            .with_status(401)
            .with_header("WWW-Authenticate", &challenge)
            .create_async()
            .await;
        let _token = server
            .mock("GET", "/token")
            .match_query(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded("service".into(), "ghcr.io".into()),
                mockito::Matcher::UrlEncoded("scope".into(), "repository:org/repo:pull".into()),
            ]))
            .with_status(200)
            .with_body(r#"{"token":"anon-token"}"#)
            .create_async()
            .await;
        let _auth = server
            .mock("HEAD", "/v2/org/repo/manifests/v1")
            .match_header("Authorization", "Bearer anon-token")
            .with_status(200)
            .with_header("Docker-Content-Digest", DIGEST)
            .create_async()
            .await;

        let res = provider_for(&server)
            .resolve_digest("ghcr.io/org/repo", "v1")
            .await
            .unwrap();
        assert_eq!(res, DIGEST);
    }

    #[tokio::test]
    async fn test_resolve_digest_falls_back_to_get() {
        let mut server = Server::new_async().await;
        let _head = server
            .mock("HEAD", "/v2/org/repo/manifests/v1")
            .with_status(405)
            .create_async()
            .await;
        let _get = server
            .mock("GET", "/v2/org/repo/manifests/v1")
            .with_status(200)
            .with_header("Docker-Content-Digest", DIGEST)
            .create_async()
            .await;

        let res = provider_for(&server)
            .resolve_digest("ghcr.io/org/repo", "v1")
            .await
            .unwrap();
        assert_eq!(res, DIGEST);
    }

    #[tokio::test]
    async fn test_resolve_digest_rejects_invalid_digest() {
        let mut server = Server::new_async().await;
        let _m = server
            .mock("HEAD", "/v2/org/repo/manifests/v1")
            .with_status(200)
            .with_header("Docker-Content-Digest", "sha256:not-a-digest")
            .create_async()
            .await;

        let err = provider_for(&server)
            .resolve_digest("ghcr.io/org/repo", "v1")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("invalid digest"));
    }

    #[tokio::test]
    async fn test_resolve_digest_error_names_image() {
        let mut server = Server::new_async().await;
        let _m = server
            .mock("HEAD", "/v2/org/repo/manifests/v1")
            .with_status(404)
            .create_async()
            .await;

        let err = provider_for(&server)
            .resolve_digest("ghcr.io/org/repo", "v1")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("ghcr.io/org/repo:v1"));
    }
}
