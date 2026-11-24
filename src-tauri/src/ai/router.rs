use super::{
    config::{ProviderConfiguration, ProviderKind},
    gemini::GeminiProvider,
    openai::OpenAIProvider,
    provider::{AIInputPart, AIProvider, AIRequest},
    usage,
};
use crate::ai::provider::AIResponse;
use tauri::AppHandle;

pub struct ModelRouter;

impl ModelRouter {
    pub fn is_ready(app: &AppHandle) -> Result<(), String> {
        let config = ProviderConfiguration::from_environment()?;
        match config.provider {
            ProviderKind::Gemini => {
                crate::config::load_gemini_api_key(app).map(|_| ())
            }
            ProviderKind::OpenAI => Err(
                "OpenAI is not enabled yet. Keep DAS_AI_PROVIDER=gemini until the OpenAI adapter is configured."
                    .to_owned(),
            ),
        }
    }

    pub async fn generate(app: &AppHandle, parts: Vec<AIInputPart>) -> Result<String, String> {
        let config = ProviderConfiguration::from_environment()?;
        let provider: Box<dyn AIProvider> = match config.provider {
            ProviderKind::Gemini => Box::new(GeminiProvider::from_app(app, config.gemini_model)?),
            ProviderKind::OpenAI => Box::new(OpenAIProvider),
        };

        let tracker = usage::global();
        tracker.record_request();
        let result: Result<AIResponse, String> = provider.generate(AIRequest::new(parts)).await;

        match result {
            Ok(response) => {
                tracker.record_success(&response.usage);
                Ok(response.text)
            }
            Err(error) => {
                tracker.record_failure();
                Err(error)
            }
        }
    }
}