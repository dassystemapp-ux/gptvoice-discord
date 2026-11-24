use std::env;

pub const DEFAULT_GEMINI_MODEL: &str = "gemini-3.1-flash-lite";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    Gemini,
    OpenAI,
}

#[derive(Debug, Clone)]
pub struct ProviderConfiguration {
    pub provider: ProviderKind,
    pub gemini_model: String,
}

impl Default for ProviderConfiguration {
    fn default() -> Self {
        Self {
            provider: ProviderKind::Gemini,
            gemini_model: DEFAULT_GEMINI_MODEL.to_owned(),
        }
    }
}

impl ProviderConfiguration {
    /// Gemini remains the active provider until OpenAI credentials and its adapter
    /// are deliberately configured. No implicit provider switch is performed.
    pub fn from_environment() -> Result<Self, String> {
        let provider = match env::var("DAS_AI_PROVIDER")
            .unwrap_or_else(|_| "gemini".to_owned())
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "gemini" | "" => ProviderKind::Gemini,
            "openai" => ProviderKind::OpenAI,
            other => return Err(format!("Unknown DAS_AI_PROVIDER value: {other}")),
        };

        let gemini_model = env::var("DAS_GEMINI_MODEL")
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| DEFAULT_GEMINI_MODEL.to_owned());

        Ok(Self {
            provider,
            gemini_model,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{ProviderConfiguration, ProviderKind, DEFAULT_GEMINI_MODEL};

    #[test]
    fn defaults_to_gemini_without_changing_existing_runtime() {
        let config = ProviderConfiguration::default();
        assert_eq!(config.provider, ProviderKind::Gemini);
        assert_eq!(config.gemini_model, DEFAULT_GEMINI_MODEL);
    }
}