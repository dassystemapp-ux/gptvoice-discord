use std::{future::Future, pin::Pin};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AIInputPart {
    Text(String),
    File { mime_type: String, bytes: Vec<u8> },
}

#[derive(Debug, Clone)]
pub struct AIRequest {
    pub parts: Vec<AIInputPart>,
    pub temperature: f32,
    pub max_output_tokens: u32,
}

impl AIRequest {
    pub fn new(parts: Vec<AIInputPart>) -> Self {
        Self {
            parts,
            temperature: 0.25,
            max_output_tokens: 1400,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
}

#[derive(Debug, Clone)]
pub struct AIResponse {
    pub text: String,
    pub usage: TokenUsage,
}

pub type ProviderFuture<'a> = Pin<Box<dyn Future<Output = Result<AIResponse, String>> + Send + 'a>>;

/// Provider-neutral contract. Callers describe content, not vendor wire formats.
pub trait AIProvider: Send + Sync {
    fn generate<'a>(&'a self, request: AIRequest) -> ProviderFuture<'a>;
}