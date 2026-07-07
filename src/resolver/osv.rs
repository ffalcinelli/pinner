use crate::error::PinnerError;
use crate::resolver::provider::{decode_cached_value, encode_cached_value};
use moka::future::Cache;
use std::path::PathBuf;
use std::time::Duration;

/// Client to query the OSV database with in-memory and on-disk caching.
pub struct OsvClient {
    client: reqwest::Client,
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

        Self {
            client: reqwest::Client::new(),
            memory_cache: Cache::builder()
                .max_capacity(1000)
                .time_to_live(memory_ttl)
                .build(),
            disk_cache_path,
            offline,
            ttl,
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
            .json(&OsvQuery {
                commit: commit.to_string(),
            })
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
}
