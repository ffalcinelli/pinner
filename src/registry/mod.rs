use crate::error::PinnerError;
use async_trait::async_trait;
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT};
use serde::Deserialize;

#[cfg(test)]
use mockall::automock;

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::Server;

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

        let manifest_path = "/v2/library/alpine/manifests/latest";
        let _m2 = server
            .mock("GET", manifest_path)
            .match_header("Authorization", "Bearer test-token")
            .with_status(200)
            .with_header("Docker-Content-Digest", "sha256:123")
            .create_async()
            .await;

        let provider = OciRegistryProvider::with_base_urls(
            format!("{}{}", server.url(), auth_path),
            format!("{}{}", server.url(), "/v2/{repository}/manifests/{tag}"),
        );

        let digest = provider.resolve_digest("alpine", "latest").await.unwrap();
        assert_eq!(digest, "sha256:123");
    }

    #[tokio::test]
    async fn test_resolve_digest_ghcr() {
        let mut server = Server::new_async().await;
        let manifest_path = "/v2/owner/repo/manifests/v1";
        let _m1 = server
            .mock("GET", manifest_path)
            .with_status(200)
            .with_header("Digest", "sha256:ghcr123")
            .create_async()
            .await;

        let provider = OciRegistryProvider::with_base_urls(
            "http://auth".to_string(),
            format!("{}{}", server.url(), "/v2/{repository}/manifests/{tag}"),
        );

        let digest = provider
            .resolve_digest("ghcr.io/owner/repo", "v1")
            .await
            .unwrap();
        assert_eq!(digest, "sha256:ghcr123");
    }

    #[tokio::test]
    async fn test_resolve_digest_invalid_token_json() {
        let mut server = Server::new_async().await;
        let _m1 = server
            .mock("GET", mockito::Matcher::Any)
            .with_status(200)
            .with_body("invalid json")
            .create_async()
            .await;

        let provider = OciRegistryProvider::with_base_urls(
            server.url(),
            format!("{}/v2/{{repository}}/manifests/{{tag}}", server.url()),
        );

        let res = provider.resolve_digest("alpine", "latest").await;
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn test_get_token_non_docker_hub() {
        let provider = OciRegistryProvider::new(None, None);
        let token = provider.get_token("ghcr.io", "owner/repo").await.unwrap();
        assert_eq!(token, "");
    }

    #[tokio::test]
    async fn test_resolve_digest_error() {
        let mut server = Server::new_async().await;
        let _m1 = server
            .mock("GET", "/v2/library/alpine/manifests/latest")
            .with_status(404)
            .create_async()
            .await;

        let provider = OciRegistryProvider::with_base_urls(
            format!("{}/auth", server.url()),
            format!("{}/v2/{{repository}}/manifests/{{tag}}", server.url()),
        );

        let res = provider.resolve_digest("alpine", "latest").await;
        assert!(res.is_err());
    }
}

/// Trait for interacting with OCI registries.
#[cfg_attr(test, automock)]
#[async_trait]
pub trait RegistryProvider: Send + Sync {
    /// Resolves a docker image tag to its digest.
    async fn resolve_digest(&self, image: &str, tag: &str) -> Result<String, PinnerError>;
}

/// Implementation of [`RegistryProvider`] for OCI-compliant registries.
#[derive(Clone)]
pub struct OciRegistryProvider {
    client: reqwest::Client,
    auth_url: String,
    base_url_template: String,
    username: Option<String>,
    password: Option<String>,
}

impl Default for OciRegistryProvider {
    fn default() -> Self {
        Self::new(None, None)
    }
}

impl OciRegistryProvider {
    pub fn new(username: Option<String>, password: Option<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            auth_url: "https://auth.docker.io/token".to_string(),
            base_url_template: "https://{registry}/v2/{repository}/manifests/{tag}".to_string(),
            username,
            password,
        }
    }

    #[cfg(test)]
    pub fn with_base_urls(auth_url: String, base_url_template: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            auth_url,
            base_url_template,
            username: None,
            password: None,
        }
    }

    async fn get_token(&self, registry: &str, repository: &str) -> Result<String, PinnerError> {
        let url = if registry == "docker.io" || registry == "registry-1.docker.io" {
            format!(
                "{}?service=registry.docker.io&scope=repository:{}:pull",
                self.auth_url, repository
            )
        } else {
            return Ok("".to_string());
        };

        let mut rb = self.client.get(&url);
        if let (Some(u), Some(p)) = (&self.username, &self.password) {
            rb = rb.basic_auth(u, Some(p));
        }

        let resp = rb
            .send()
            .await
            .map_err(|e| PinnerError::Api(e.to_string()))?;

        #[derive(Deserialize)]
        struct TokenResponse {
            token: String,
        }

        let res: TokenResponse = resp
            .json()
            .await
            .map_err(|e| PinnerError::Api(e.to_string()))?;
        Ok(res.token)
    }
}

#[async_trait]
impl RegistryProvider for OciRegistryProvider {
    async fn resolve_digest(&self, image: &str, tag: &str) -> Result<String, PinnerError> {
        let (registry, repository) = if let Some(pos) = image.find('/') {
            let first_part = &image[..pos];
            if first_part.contains('.') || first_part.contains(':') || first_part == "localhost" {
                (first_part, &image[pos + 1..])
            } else {
                ("registry-1.docker.io", image)
            }
        } else {
            ("registry-1.docker.io", image)
        };

        // Standard Docker Hub library images
        let full_repo = if registry == "registry-1.docker.io" && !repository.contains('/') {
            format!("library/{}", repository)
        } else {
            repository.to_string()
        };

        let token = self.get_token(registry, &full_repo).await?;

        let url = self
            .base_url_template
            .replace("{registry}", registry)
            .replace("{repository}", &full_repo)
            .replace("{tag}", tag);

        let mut headers = HeaderMap::new();
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/vnd.oci.image.manifest.v1+json"),
        );
        headers.append(
            ACCEPT,
            HeaderValue::from_static("application/vnd.docker.distribution.manifest.v2+json"),
        );

        if !token.is_empty() {
            headers.insert(
                "Authorization",
                HeaderValue::from_str(&format!("Bearer {}", token))
                    .map_err(|e| PinnerError::Api(e.to_string()))?,
            );
        } else if let (Some(u), Some(p)) = (&self.username, &self.password) {
            let auth = format!("{}:{}", u, p);
            headers.insert(
                "Authorization",
                HeaderValue::from_str(&format!("Basic {}", b64_encode(&auth)))
                    .map_err(|e| PinnerError::Api(e.to_string()))?,
            );
        }

        let resp = self
            .client
            .get(&url)
            .headers(headers)
            .send()
            .await
            .map_err(|e| PinnerError::Api(e.to_string()))?;

        if resp.status().is_success() {
            let digest = resp
                .headers()
                .get("Docker-Content-Digest")
                .or_else(|| resp.headers().get("Digest"))
                .and_then(|h| h.to_str().ok())
                .ok_or_else(|| PinnerError::Api("Digest not found in response headers".into()))?;
            Ok(digest.to_string())
        } else {
            Err(PinnerError::Api(format!(
                "Failed to fetch manifest: HTTP {}",
                resp.status()
            )))
        }
    }
}

fn b64_encode(s: &str) -> String {
    use base64::{engine::general_purpose, Engine as _};
    general_purpose::STANDARD.encode(s)
}
