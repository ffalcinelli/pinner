//! The resolver module is responsible for mapping symbolic dependency references
//! (like `@v3` or `:latest`) to immutable hashes (like SHA-1 or digests).
//!
//! It implements various specialized clients for different CI/CD platforms
//! (GitHub, GitLab, Bitbucket, etc.) and OCI registries. These clients are
//! orchestrated by a `UnifiedProvider` and a high-level `Resolver`.

pub mod azure;
pub mod bitbucket;
pub mod circleci;
pub mod forgejo;
pub mod github;
pub mod gitlab;
pub mod osv;
pub mod provider;
pub mod registry;
pub mod unified;

pub use azure::ReqwestAzureProvider;
pub use bitbucket::ReqwestBitbucketProvider;
pub use circleci::ReqwestCircleCiProvider;
pub use osv::OsvClient;
pub use provider::{
    CachedProvider, ProviderType, RemoteProvider, UnifiedProvider, UnifiedProviderConfig,
};
pub use registry::{OciRegistryProvider, RegistryProvider};
pub use unified::Resolver;
