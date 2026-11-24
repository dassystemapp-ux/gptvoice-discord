use super::provider::{AIInputPart, AIProvider, AIRequest, AIResponse, ProviderFuture, TokenUsage};
use crate::config;
use base64::Engine as _;
use serde_json::{json, Value};
use std::time::Duration;
use tauri::AppHandle;

const MAX_ATTEMPTS: u32 = 4;

fn retry_delay(attempt: u32) -> Duration {
    Duration::from_millis(500 * (1_u64 << attempt.min(2)))
}

pub struct GeminiProvider {
    client: reqwest::Client,
    api_key: String,
    model: String,
}

impl GeminiProvider {
    pub fn from_app(app: &AppHandle, model: impl Into<String>) -> Result<Self, String> {
        let api_key = config::load_gemini_api_key(app).map_err(|_| {
            "تعذر العثور على مفتاح Gemini. تحقق من ملف API GEMINI.txt داخل مجلد التنزيلات."
                .to_owned()
        })?;
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(35))
            .build()
            .map_err(|_| "تعذر تهيئة اتصال المساعد.".to_owned())?;

        Ok(Self {
            client,
            api_key,
            model: model.into(),
        })
    }
}

fn gemini_parts(parts: Vec<AIInputPart>) -> Vec<Value> {
    parts
        .into_iter()
        .map(|part| match part {
            AIInputPart::Text(text) => json!({"text": text}),
            AIInputPart::File { mime_type, bytes } => json!({
                "inline_data": {
                    "mime_type": mime_type,
                    "data": base64::engine::general_purpose::STANDARD.encode(bytes)
                }
            }),
        })
        .collect()
}

impl AIProvider for GeminiProvider {
    fn generate<'a>(&'a self, request: AIRequest) -> ProviderFuture<'a> {
        Box::pin(async move {
            let endpoint = format!(
                "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent",
                self.model
            );
            let AIRequest {
                parts,
                temperature,
                max_output_tokens,
            } = request;
            let payload = json!({
                "contents": [{"role": "user", "parts": gemini_parts(parts)}],
                "generationConfig": {
                    "temperature": temperature,
                    "maxOutputTokens": max_output_tokens
                }
            });

            for attempt in 0..MAX_ATTEMPTS {
                let response = match self
                    .client
                    .post(&endpoint)
                    .header("x-goog-api-key", self.api_key.as_str())
                    .json(&payload)
                    .send()
                    .await
                {
                    Ok(response) => response,
                    Err(error)
                        if attempt + 1 < MAX_ATTEMPTS
                            && (error.is_timeout() || error.is_connect()) =>
                    {
                        tokio::time::sleep(retry_delay(attempt)).await;
                        continue;
                    }
                    Err(_) => {
                        return Err("تعذر الاتصال بخدمة Gemini. حاول مرة أخرى لاحقًا.".to_owned());
                    }
                };

                let status = response.status();
                if status.is_server_error() && attempt + 1 < MAX_ATTEMPTS {
                    tokio::time::sleep(retry_delay(attempt)).await;
                    continue;
                }

                let body: Value = response.json().await.unwrap_or(Value::Null);
                if !status.is_success() {
                    return Err(if status.as_u16() == 429 {
                        "وصلنا إلى حد استخدام Gemini المجاني مؤقتًا. جرّب لاحقًا.".to_owned()
                    } else if status.as_u16() == 401 || status.as_u16() == 403 {
                        "مفتاح Gemini غير صالح أو لا يملك صلاحية النموذج. راجع إعدادات API."
                            .to_owned()
                    } else {
                        format!("تعذر التحليل عبر Gemini (HTTP {}).", status.as_u16())
                    });
                }

                let text = body["candidates"][0]["content"]["parts"]
                    .as_array()
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(|item| item["text"].as_str())
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .unwrap_or_default();

                if text.trim().is_empty() {
                    return Err(
                        "لم يتمكن Gemini من قراءة السؤال بوضوح. أرسل صورة أوضح أو اكتب نص السؤال."
                            .to_owned(),
                    );
                }

                let usage = TokenUsage {
                    input_tokens: body["usageMetadata"]["promptTokenCount"]
                        .as_u64()
                        .unwrap_or_default(),
                    output_tokens: body["usageMetadata"]["candidatesTokenCount"]
                        .as_u64()
                        .unwrap_or_default(),
                    total_tokens: body["usageMetadata"]["totalTokenCount"]
                        .as_u64()
                        .unwrap_or_default(),
                };

                return Ok(AIResponse {
                    text: text.trim().to_owned(),
                    usage,
                });
            }

            Err("تعذر التحليل عبر Gemini بعد إعادة محاولة محدودة.".to_owned())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{gemini_parts, retry_delay, AIInputPart};
    use serde_json::json;
    use std::time::Duration;

    #[test]
    fn transient_failures_use_bounded_exponential_backoff() {
        assert_eq!(retry_delay(0), Duration::from_millis(500));
        assert_eq!(retry_delay(1), Duration::from_millis(1_000));
        assert_eq!(retry_delay(2), Duration::from_millis(2_000));
    }

    #[test]
    fn translates_neutral_text_and_file_inputs_to_gemini_wire_format() {
        let parts = gemini_parts(vec![
            AIInputPart::Text("Explain this".to_owned()),
            AIInputPart::File {
                mime_type: "image/png".to_owned(),
                bytes: vec![0, 1, 2],
            },
        ]);

        assert_eq!(parts[0], json!({"text":"Explain this"}));
        assert_eq!(parts[1]["inline_data"]["mime_type"], "image/png");
        assert_eq!(parts[1]["inline_data"]["data"], "AAEC");
    }
}