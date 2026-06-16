pub mod azure;
pub mod bitbucket;
pub mod circleci;
pub mod forgejo;
pub mod github;
pub mod gitlab;
pub mod provider;
pub mod registry;
pub mod unified;

pub use azure::ReqwestAzureProvider;
pub use bitbucket::ReqwestBitbucketProvider;
pub use circleci::ReqwestCircleCiProvider;
pub use provider::{ProviderType, RemoteProvider, UnifiedProvider, UnifiedProviderConfig};
pub use registry::{OciRegistryProvider, RegistryProvider};
pub use unified::Resolver;
