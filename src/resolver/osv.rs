use crate::error::PinnerError;
use crate::resolver::provider::{decode_cached_value, encode_cached_value};
use moka::future::Cache;
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use reqwest_retry::{policies::ExponentialBackoff, RetryTransientMiddleware};
use std::path::PathBuf;
use std::time::Duration;

/// How OSV advisories classify a commit.
///
/// Ordered by severity, so the worst verdict of several advisories is their maximum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum OsvVerdict {
    /// No advisories.
    Clean,
    /// Ordinary vulnerabilities (CVEs and similar).
    Vulnerable,
    /// A malicious or hijacked release (supply-chain compromise).
    Compromised,
}

/// A single OSV advisory affecting a commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OsvAdvisory {
    /// Advisory identifier (e.g. `GHSA-…`, `MAL-…`).
    pub id: String,
    /// One-line summary, empty if OSV provides none.
    pub summary: String,
    /// Whether the advisory describes a supply-chain compromise.
    pub compromise: bool,
}

/// The classified OSV result for a commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OsvAssessment {
    /// The most severe verdict across all advisories.
    pub verdict: OsvVerdict,
    /// Every advisory returned by OSV.
    pub advisories: Vec<OsvAdvisory>,
}

impl OsvAssessment {
    /// An assessment with no advisories.
    pub fn clean() -> Self {
        Self {
            verdict: OsvVerdict::Clean,
            advisories: Vec::new(),
        }
    }

    /// Advisory identifiers, for compact reporting.
    pub fn ids(&self) -> Vec<String> {
        self.advisories.iter().map(|a| a.id.clone()).collect()
    }
}

/// Words in an OSV advisory that indicate a supply-chain compromise rather than a
/// regular vulnerability.
const COMPROMISE_KEYWORDS: [&str; 6] = [
    "malicious",
    "compromised",
    "backdoor",
    "malware",
    "hijacked",
    "exfiltrat",
];

/// Classifies an OSV `/v1/query` response body.
///
/// An advisory is a compromise when it comes from OSV's malicious-packages database
/// (`MAL-` identifiers) or its summary/details mention malicious, backdoored,
/// hijacked or exfiltrating code. Any other advisory is an ordinary vulnerability.
/// `verify` and `scan` both use this, so they always agree.
pub fn assess_osv_response(body: &str) -> Result<OsvAssessment, PinnerError> {
    #[derive(serde::Deserialize)]
    struct OsvResponse {
        vulns: Option<Vec<OsvVulnerability>>,
    }

    #[derive(serde::Deserialize)]
    struct OsvVulnerability {
        id: String,
        summary: Option<String>,
        details: Option<String>,
    }

    let response: OsvResponse = serde_json::from_str(body)
        .map_err(|e| PinnerError::Api(format!("Invalid OSV response: {}", e)))?;

    let mut assessment = OsvAssessment::clean();
    for vuln in response.vulns.unwrap_or_default() {
        let summary = vuln.summary.unwrap_or_default();
        let text = format!("{} {}", summary, vuln.details.unwrap_or_default()).to_lowercase();
        let compromise =
            vuln.id.starts_with("MAL-") || COMPROMISE_KEYWORDS.iter().any(|k| text.contains(k));
        let verdict = if compromise {
            OsvVerdict::Compromised
        } else {
            OsvVerdict::Vulnerable
        };
        assessment.verdict = assessment.verdict.max(verdict);
        assessment.advisories.push(OsvAdvisory {
            id: vuln.id,
            summary,
            compromise,
        });
    }
    Ok(assessment)
}

/// Client to query the OSV database with in-memory and on-disk caching.
pub struct OsvClient {
    client: ClientWithMiddleware,
    memory_cache: Cache<String, String>,
    disk_cache_path: Option<PathBuf>,
    offline: bool,
    ttl: Duration,
}

impl OsvClient {
    /// Creates a new `OsvClient`.
    pub fn new(disk_cache_path: Option<PathBuf>, offline: bool, ttl: Duration) -> Self {
        let memory_ttl = if ttl > Duration::from_secs(0) {
            ttl
        } else {
            Duration::from_secs(1)
        };

        let client = reqwest::Client::builder()
            .user_agent("pinner")
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        let retry_policy = ExponentialBackoff::builder().build_with_max_retries(3);

        Self {
            client: ClientBuilder::new(client)
                .with(RetryTransientMiddleware::new_with_policy(retry_policy))
                .build(),
            memory_cache: Cache::builder()
                .max_capacity(1000)
                .time_to_live(memory_ttl)
                .build(),
            disk_cache_path,
            offline,
            ttl,
        }
    }

    /// Queries OSV for a commit and classifies the advisories.
    pub async fn assess_commit(&self, commit: &str) -> Result<OsvAssessment, PinnerError> {
        match self.query_commit(commit).await? {
            Some(body) => assess_osv_response(&body),
            None => Ok(OsvAssessment::clean()),
        }
    }

    /// Queries OSV for vulnerability info for a given commit SHA.
    ///
    /// If caching is enabled and a cache entry exists, returns the cached JSON string.
    pub async fn query_commit(&self, commit: &str) -> Result<Option<String>, PinnerError> {
        let mem_key = commit.to_string();

        if self.ttl > Duration::from_secs(0) {
            if let Some(cached) = self.memory_cache.get(&mem_key).await {
                return Ok(Some(cached));
            }

            // Try disk cache
            let disk_key = format!("osv:commit:{}", commit);
            if let Some(path) = &self.disk_cache_path {
                if let Ok(data) = cacache::read(path, &disk_key).await {
                    if let Some(val) = decode_cached_value(&data, self.ttl) {
                        self.memory_cache.insert(mem_key.clone(), val.clone()).await;
                        return Ok(Some(val));
                    }
                }
            }
        }

        if self.offline {
            return Err(PinnerError::Offline(format!(
                "Network request to OSV for commit {} is disabled in offline mode",
                commit
            )));
        }

        let base_url = std::env::var("PINNER_OSV_URL")
            .unwrap_or_else(|_| "https://api.osv.dev/v1/query".to_string());

        #[derive(serde::Serialize)]
        struct OsvQuery {
            commit: String,
        }

        let response = self
            .client
            .post(&base_url)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(
                serde_json::to_vec(&OsvQuery {
                    commit: commit.to_string(),
                })
                .map_err(|e| PinnerError::Api(e.to_string()))?,
            )
            .send()
            .await
            .map_err(|e| PinnerError::Api(format!("Failed to send OSV request: {}", e)))?;

        if !response.status().is_success() {
            return Err(PinnerError::Api(format!(
                "OSV API returned error status: {}",
                response.status()
            )));
        }

        let body_str = response
            .text()
            .await
            .map_err(|e| PinnerError::Api(format!("Failed to read OSV response body: {}", e)))?;

        // Update caches
        if self.ttl > Duration::from_secs(0) {
            self.memory_cache
                .insert(mem_key.clone(), body_str.clone())
                .await;
            if let Some(path) = &self.disk_cache_path {
                let disk_key = format!("osv:commit:{}", commit);
                let encoded = encode_cached_value(&body_str);
                let _ = cacache::write(path, &disk_key, encoded.as_bytes()).await;
            }
        }

        Ok(Some(body_str))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::Server;
    use tempfile::tempdir;

    #[tokio::test]
    #[serial_test::serial]
    async fn test_osv_query_uncached() {
        let mut server = Server::new_async().await;
        let response_body = r#"{"vulns":[]}"#;

        let _m = server
            .mock("POST", "/")
            .match_body(mockito::Matcher::JsonString(
                r#"{"commit":"hash123"}"#.to_string(),
            ))
            .with_status(200)
            .with_body(response_body)
            .create_async()
            .await;

        std::env::set_var("PINNER_OSV_URL", server.url());

        let client = OsvClient::new(None, false, Duration::from_secs(0));
        let res = client.query_commit("hash123").await.unwrap().unwrap();
        assert_eq!(res, response_body);

        std::env::remove_var("PINNER_OSV_URL");
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_osv_query_cached_memory() {
        let mut server = Server::new_async().await;
        let response_body = r#"{"vulns":[{"id":"VULN-1"}]}"#;

        let _m = server
            .mock("POST", "/")
            .match_body(mockito::Matcher::JsonString(
                r#"{"commit":"hash123"}"#.to_string(),
            ))
            .with_status(200)
            .with_body(response_body)
            .expect(1) // Request should only be made ONCE
            .create_async()
            .await;

        std::env::set_var("PINNER_OSV_URL", server.url());

        let client = OsvClient::new(None, false, Duration::from_secs(3600));

        // First call - misses cache, hits server
        let res1 = client.query_commit("hash123").await.unwrap().unwrap();
        assert_eq!(res1, response_body);

        // Second call - hits memory cache
        let res2 = client.query_commit("hash123").await.unwrap().unwrap();
        assert_eq!(res2, response_body);

        std::env::remove_var("PINNER_OSV_URL");
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_osv_query_cached_disk() {
        let mut server = Server::new_async().await;
        let response_body = r#"{"vulns":[]}"#;

        let _m = server
            .mock("POST", "/")
            .match_body(mockito::Matcher::JsonString(
                r#"{"commit":"hash_disk"}"#.to_string(),
            ))
            .with_status(200)
            .with_body(response_body)
            .expect(1) // Request should only be made ONCE
            .create_async()
            .await;

        std::env::set_var("PINNER_OSV_URL", server.url());

        let tmp = tempdir().unwrap();
        let client1 = OsvClient::new(
            Some(tmp.path().to_path_buf()),
            false,
            Duration::from_secs(3600),
        );

        // First call - misses cache, writes to disk
        let res1 = client1.query_commit("hash_disk").await.unwrap().unwrap();
        assert_eq!(res1, response_body);

        // Create a new client pointing to the same disk path to bypass memory cache
        let client2 = OsvClient::new(
            Some(tmp.path().to_path_buf()),
            false,
            Duration::from_secs(3600),
        );

        // Second call - hits disk cache
        let res2 = client2.query_commit("hash_disk").await.unwrap().unwrap();
        assert_eq!(res2, response_body);

        std::env::remove_var("PINNER_OSV_URL");
    }

    #[tokio::test]
    async fn test_osv_query_offline_error() {
        let client = OsvClient::new(None, true, Duration::from_secs(3600));
        let res = client.query_commit("hash123").await;
        assert!(res.is_err());
        assert!(matches!(res.unwrap_err(), PinnerError::Offline(_)));
    }

    #[test]
    fn test_assess_osv_response() {
        assert_eq!(
            assess_osv_response(r#"{"vulns":[]}"#).unwrap(),
            OsvAssessment::clean()
        );
        assert_eq!(assess_osv_response("{}").unwrap(), OsvAssessment::clean());

        let cve =
            assess_osv_response(r#"{"vulns":[{"id":"GHSA-1","summary":"Denial of service"}]}"#)
                .unwrap();
        assert_eq!(cve.verdict, OsvVerdict::Vulnerable);
        assert_eq!(cve.ids(), vec!["GHSA-1"]);
        assert!(!cve.advisories[0].compromise);

        let mixed = assess_osv_response(
            r#"{"vulns":[
                {"id":"GHSA-1","summary":"Denial of service"},
                {"id":"GHSA-2","summary":"Release","details":"The tag was HIJACKED to exfiltrate secrets"}
            ]}"#,
        )
        .unwrap();
        assert_eq!(mixed.verdict, OsvVerdict::Compromised);
        assert!(mixed.advisories[1].compromise);

        let mal = assess_osv_response(r#"{"vulns":[{"id":"MAL-2025-1"}]}"#).unwrap();
        assert_eq!(mal.verdict, OsvVerdict::Compromised);

        assert!(assess_osv_response("not json").is_err());
    }
}
