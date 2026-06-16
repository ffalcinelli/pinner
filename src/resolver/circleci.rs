use crate::core::{BranchName, DependencyName, DependencyRef};
use crate::error::PinnerError;
use crate::resolver::provider::RemoteProvider;
use async_trait::async_trait;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, USER_AGENT};
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use reqwest_retry::policies::ExponentialBackoff;
use reqwest_retry::RetryTransientMiddleware;
use serde::{Deserialize, Serialize};

/// Implementation of [`RemoteProvider`] for CircleCI Orbs using the GraphQL API.
pub struct ReqwestCircleCiProvider {
    pub client: ClientWithMiddleware,
    pub base_url: String,
}

impl ReqwestCircleCiProvider {
    pub fn new(base_url: String, token: Option<String>) -> Result<Self, PinnerError> {
        let mut h = HeaderMap::new();
        h.insert(USER_AGENT, HeaderValue::from_static("pinner"));

        let token = token.or_else(|| std::env::var("CIRCLECI_TOKEN").ok());

        if let Some(t) = token {
            // CircleCI GraphQL API supports both Circle-Token header and Authorization header.
            // We use Authorization for consistency with other providers if possible,
            // but many CircleCI examples use Circle-Token.
            if let Ok(auth) = HeaderValue::from_str(&t) {
                h.insert(AUTHORIZATION, auth);
                if let Ok(circle_token) = HeaderValue::from_str(&t) {
                    h.insert("Circle-Token", circle_token);
                }
            }
        }

        let reqwest_client = reqwest::Client::builder()
            .default_headers(h)
            .build()
            .map_err(|e| PinnerError::Api(format!("Failed to build reqwest client: {}", e)))?;

        let retry_policy = ExponentialBackoff::builder().build_with_max_retries(3);
        let client = ClientBuilder::new(reqwest_client)
            .with(RetryTransientMiddleware::new_with_policy(retry_policy))
            .build();

        Ok(Self { client, base_url })
    }
}

#[derive(Serialize)]
struct GraphQLRequest {
    query: String,
    variables: serde_json::Value,
}

#[derive(Deserialize)]
struct GraphQLResponse<T> {
    data: Option<T>,
    #[allow(dead_code)]
    errors: Option<Vec<GraphQLError>>,
}

#[derive(Deserialize)]
struct GraphQLError {
    #[allow(dead_code)]
    message: String,
}

#[derive(Deserialize)]
struct OrbData {
    orb: Option<OrbVersions>,
}

#[derive(Deserialize)]
struct OrbVersions {
    versions: Vec<OrbVersion>,
}

#[derive(Deserialize)]
struct OrbVersion {
    version: String,
}

#[async_trait]
impl RemoteProvider for ReqwestCircleCiProvider {
    async fn get_commit_sha(
        &self,
        _action: &DependencyName,
        _tag: &str,
        _key: &str,
    ) -> Result<DependencyRef, PinnerError> {
        Err(PinnerError::Unsupported(
            "CircleCI Orbs do not support Git SHA pinning".to_string(),
        ))
    }

    async fn get_latest_release(
        &self,
        action: &DependencyName,
        _key: &str,
    ) -> Result<String, PinnerError> {
        let query = r#"
            query GetOrb($name: String!) {
                orb(name: $name) {
                    versions(count: 1) {
                        version
                    }
                }
            }
        "#;

        let variables = serde_json::json!({ "name": action.0 });
        let body = serde_json::to_vec(&GraphQLRequest {
            query: query.to_string(),
            variables,
        })
        .map_err(|e| PinnerError::Api(e.to_string()))?;

        let resp = self
            .client
            .post(&self.base_url)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body)
            .send()
            .await
            .map_err(|e| PinnerError::Api(e.to_string()))?;

        if resp.status().is_success() {
            let res: GraphQLResponse<OrbData> = resp
                .json()
                .await
                .map_err(|e| PinnerError::Api(e.to_string()))?;
            if let Some(data) = res.data {
                if let Some(orb) = data.orb {
                    if let Some(v) = orb.versions.first() {
                        return Ok(v.version.clone());
                    }
                }
            }
            Err(PinnerError::Api(format!("Orb not found: {}", action)))
        } else {
            Err(PinnerError::Api(format!(
                "CircleCI API error (HTTP {}): {}",
                resp.status(),
                action
            )))
        }
    }

    async fn list_tags(
        &self,
        action: &DependencyName,
        _key: &str,
    ) -> Result<Vec<String>, PinnerError> {
        let query = r#"
            query GetOrbVersions($name: String!) {
                orb(name: $name) {
                    versions(count: 100) {
                        version
                    }
                }
            }
        "#;

        let variables = serde_json::json!({ "name": action.0 });
        let body = serde_json::to_vec(&GraphQLRequest {
            query: query.to_string(),
            variables,
        })
        .map_err(|e| PinnerError::Api(e.to_string()))?;

        let resp = self
            .client
            .post(&self.base_url)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body)
            .send()
            .await
            .map_err(|e| PinnerError::Api(e.to_string()))?;

        if resp.status().is_success() {
            let res: GraphQLResponse<OrbData> = resp
                .json()
                .await
                .map_err(|e| PinnerError::Api(e.to_string()))?;
            if let Some(data) = res.data {
                if let Some(orb) = data.orb {
                    return Ok(orb.versions.into_iter().map(|v| v.version).collect());
                }
            }
            Err(PinnerError::Api(format!("Orb not found: {}", action)))
        } else {
            Err(PinnerError::Api(format!(
                "CircleCI API error (HTTP {}): {}",
                resp.status(),
                action
            )))
        }
    }

    async fn get_default_branch(
        &self,
        _action: &DependencyName,
        _key: &str,
    ) -> Result<BranchName, PinnerError> {
        Err(PinnerError::Unsupported(
            "CircleCI Orbs do not have default branches".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_circleci_get_latest_release() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("POST", "/")
            .with_status(200)
            .with_body(r#"{"data":{"orb":{"versions":[{"version":"5.1.0"}]}}}"#)
            .create_async()
            .await;

        let provider = ReqwestCircleCiProvider::new(server.url(), None).unwrap();
        let tag = provider
            .get_latest_release(&DependencyName::from("circleci/node"), "orbs")
            .await
            .unwrap();
        assert_eq!(tag, "5.1.0");
    }
}
