use super::provider::{AIProvider, AIRequest, ProviderFuture};

/// Reserved adapter for the future DAS AI Core migration.
/// It intentionally returns a configuration error until a real OpenAI
/// implementation, key management, and integration tests are supplied.
pub struct OpenAIProvider;

impl AIProvider for OpenAIProvider {
    fn generate<'a>(&'a self, _request: AIRequest) -> ProviderFuture<'a> {
        Box::pin(async {
            Err(
                "OpenAI provider is prepared but not enabled. Configure credentials and complete the adapter before switching."
                    .to_owned(),
            )
        })
    }
}