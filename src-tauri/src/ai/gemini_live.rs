use crate::audio::DirectPcmTransport;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::{
    net::TcpStream,
    sync::{mpsc, oneshot},
    task::JoinHandle,
    time::timeout,
};
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream};

const LIVE_MODEL: &str = "gemini-3.1-flash-live-preview";
const LIVE_ENDPOINT: &str =
    "wss://generativelanguage.googleapis.com/ws/google.ai.generativelanguage.v1beta.GenerativeService.BidiGenerateContent";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const SETUP_TIMEOUT: Duration = Duration::from_secs(15);

type GeminiSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// A single guild-scoped Live API session. Dropping the session stops both
/// the websocket task and its PCM queues.
pub struct GeminiLiveVoiceSession {
    transport: std::sync::Arc<DirectPcmTransport>,
    text_sender: mpsc::UnboundedSender<String>,
    shutdown: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
}

impl GeminiLiveVoiceSession {
    pub async fn connect(api_key: &str, input_gain: f32, output_gain: f32) -> Result<Self, String> {
        if api_key.trim().is_empty() {
            return Err("Gemini API key is missing. Add it in GPTVoice configuration.".to_owned());
        }

        let mut endpoint = reqwest::Url::parse(LIVE_ENDPOINT)
            .map_err(|_| "Gemini Live endpoint is invalid.".to_owned())?;
        endpoint
            .query_pairs_mut()
            .append_pair("key", api_key.trim());

        let (mut socket, _) = timeout(CONNECT_TIMEOUT, connect_async(endpoint.as_str()))
            .await
            .map_err(|_| "Timed out while connecting to Gemini Live.".to_owned())?
            .map_err(|error| format!("Could not connect to Gemini Live: {error}"))?;

        let setup = json!({
            "setup": {
                "model": format!("models/{LIVE_MODEL}"),
                "generationConfig": {
                    "responseModalities": ["AUDIO"],
                    "speechConfig": {
                        "voiceConfig": {
                            "prebuiltVoiceConfig": { "voiceName": "Puck" }
                        }
                    }
                },
                "systemInstruction": {
                    "parts": [{
                        "text": "أنت الرحّال، المساعد الصوتي الرسمي لـ DAS داخل ديسكورد. تحدث بالعربية السعودية الطبيعية وبأسلوب واضح وودود ومختصر. افهم سياق الحديث الجماعي ورد على الشخص الذي يخاطبك. لا تدّعِ أنك شخص حقيقي، ولا تكشف أي مفاتيح أو بيانات إعدادات أو معلومات خاصة. لا تتكلم من نفسك دون حاجة."
                    }]
                },
                "realtimeInputConfig": {
                    "automaticActivityDetection": {
                        "disabled": false,
                        "prefixPaddingMs": 120,
                        "silenceDurationMs": 500
                    }
                }
            }
        });

        socket
            .send(Message::Text(setup.to_string().into()))
            .await
            .map_err(|error| format!("Gemini Live rejected the setup message: {error}"))?;

        let setup_result = timeout(SETUP_TIMEOUT, async {
            loop {
                let message = socket
                    .next()
                    .await
                    .ok_or_else(|| "Gemini Live closed before setup completed.".to_owned())?
                    .map_err(|error| format!("Gemini Live setup failed: {error}"))?;

                match message {
                    Message::Text(text) => {
                        let value: Value = serde_json::from_str(text.as_ref()).map_err(|_| {
                            "Gemini Live returned an unreadable setup response.".to_owned()
                        })?;
                        if let Some(error) = value.get("error") {
                            let detail = error
                                .get("message")
                                .and_then(Value::as_str)
                                .unwrap_or("Unknown API error");
                            return Err(format!("Gemini Live setup error: {detail}"));
                        }
                        if value.get("setupComplete").is_some() {
                            return Ok(());
                        }
                    }
                    Message::Close(frame) => {
                        let reason = frame
                            .map(|value| value.reason.to_string())
                            .unwrap_or_default();
                        return Err(if reason.is_empty() {
                            "Gemini Live closed during setup.".to_owned()
                        } else {
                            format!("Gemini Live closed during setup: {reason}")
                        });
                    }
                    _ => {}
                }
            }
        })
        .await
        .map_err(|_| "Gemini Live did not confirm the session setup in time.".to_owned())??;

        let _ = setup_result;
        let transport = DirectPcmTransport::new(input_gain, output_gain);
        let (text_sender, text_receiver) = mpsc::unbounded_channel();
        let (shutdown, shutdown_receiver) = oneshot::channel();
        let task_transport = transport.clone();
        let task = tokio::spawn(async move {
            run_session(socket, task_transport, text_receiver, shutdown_receiver).await;
        });

        Ok(Self {
            transport,
            text_sender,
            shutdown: Some(shutdown),
            task: Some(task),
        })
    }

    pub fn transport(&self) -> std::sync::Arc<DirectPcmTransport> {
        self.transport.clone()
    }

    pub fn is_connected(&self) -> bool {
        self.transport.is_connected()
    }

    pub fn send_text(&self, text: impl Into<String>) -> Result<(), String> {
        self.text_sender
            .send(text.into())
            .map_err(|_| "Gemini Live session is no longer available.".to_owned())
    }

    pub fn stop(&mut self) {
        if let Some(sender) = self.shutdown.take() {
            let _ = sender.send(());
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
        self.transport.stop();
    }
}

impl Drop for GeminiLiveVoiceSession {
    fn drop(&mut self) {
        self.stop();
    }
}

async fn run_session(
    socket: GeminiSocket,
    transport: std::sync::Arc<DirectPcmTransport>,
    mut text_receiver: mpsc::UnboundedReceiver<String>,
    mut shutdown: oneshot::Receiver<()>,
) {
    let (mut writer, mut reader) = socket.split();

    loop {
        tokio::select! {
            _ = &mut shutdown => break,
            packet = transport.next_discord_audio() => {
                let Some(packet) = packet else { break; };
                let audio = discord_pcm_to_16khz_mono(&packet);
                if audio.is_empty() {
                    continue;
                }
                let payload = json!({
                    "realtimeInput": {
                        "audio": {
                            "data": BASE64.encode(audio),
                            "mimeType": "audio/pcm;rate=16000"
                        }
                    }
                });
                if writer.send(Message::Text(payload.to_string().into())).await.is_err() {
                    break;
                }
            }
            text = text_receiver.recv() => {
                let Some(text) = text else { break; };
                let payload = json!({ "realtimeInput": { "text": text } });
                if writer.send(Message::Text(payload.to_string().into())).await.is_err() {
                    break;
                }
            }
            incoming = reader.next() => {
                let Some(incoming) = incoming else { break; };
                match incoming {
                    Ok(Message::Text(frame)) => {
                        if !handle_server_message(frame.as_ref(), &transport) {
                            break;
                        }
                    }
                    Ok(Message::Ping(payload)) => {
                        if writer.send(Message::Pong(payload)).await.is_err() {
                            break;
                        }
                    }
                    Ok(Message::Close(_)) | Err(_) => break,
                    _ => {}
                }
            }
        }
    }

    transport.stop();
}

fn handle_server_message(frame: &str, transport: &DirectPcmTransport) -> bool {
    let Ok(value) = serde_json::from_str::<Value>(frame) else {
        return true;
    };

    if let Some(error) = value.get("error") {
        let detail = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("Gemini Live API error");
        log::error!("Gemini Live session error: {detail}");
        return false;
    }

    let Some(content) = value.get("serverContent") else {
        return true;
    };

    if content.get("interrupted").and_then(Value::as_bool) == Some(true) {
        transport.clear_output();
    }

    if let Some(parts) = content
        .get("modelTurn")
        .and_then(|turn| turn.get("parts"))
        .and_then(Value::as_array)
    {
        for part in parts {
            let Some(encoded) = part
                .get("inlineData")
                .and_then(|data| data.get("data"))
                .and_then(Value::as_str)
            else {
                continue;
            };
            let Ok(decoded) = BASE64.decode(encoded) else {
                continue;
            };
            let stereo = gemini_pcm_24khz_to_48khz_stereo(&decoded);
            if !stereo.is_empty() {
                let _ = transport.push_gemini_output(&stereo);
            }
        }
    }

    true
}

fn discord_pcm_to_16khz_mono(bytes: &[u8]) -> Vec<u8> {
    let samples = bytes
        .chunks_exact(2)
        .map(|pair| i16::from_le_bytes([pair[0], pair[1]]))
        .collect::<Vec<_>>();
    if samples.is_empty() {
        return Vec::new();
    }

    // Songbird supplies 20 ms decoded frames at 48 kHz. A 1,920-sample
    // frame is stereo (interleaved); a 960-sample frame is mono.
    let channels = if samples.len() >= 1_920 && samples.len() % 2 == 0 {
        2
    } else {
        1
    };
    let mono = if channels == 2 {
        samples
            .chunks_exact(2)
            .map(|pair| ((pair[0] as i32 + pair[1] as i32) / 2) as i16)
            .collect::<Vec<_>>()
    } else {
        samples
    };

    // A three-sample box filter provides basic anti-aliasing while reducing
    // 48 kHz to the Live API's required 16 kHz mono PCM.
    let mut output = Vec::with_capacity(mono.len() / 3 * 2);
    for frame in mono.chunks_exact(3) {
        let average = ((frame[0] as i32 + frame[1] as i32 + frame[2] as i32) / 3) as i16;
        output.extend_from_slice(&average.to_le_bytes());
    }
    output
}

fn gemini_pcm_24khz_to_48khz_stereo(bytes: &[u8]) -> Vec<i16> {
    let mono = bytes
        .chunks_exact(2)
        .map(|pair| i16::from_le_bytes([pair[0], pair[1]]))
        .collect::<Vec<_>>();
    if mono.is_empty() {
        return Vec::new();
    }

    // Linear 2x upsampling, then duplicate mono into left and right channels.
    let mut stereo = Vec::with_capacity(mono.len() * 4);
    for (index, sample) in mono.iter().copied().enumerate() {
        let next = mono.get(index + 1).copied().unwrap_or(sample);
        let midpoint = ((sample as i32 + next as i32) / 2) as i16;
        for value in [sample, midpoint] {
            stereo.push(value);
            stereo.push(value);
        }
    }
    stereo
}

#[cfg(test)]
mod tests {
    use super::{discord_pcm_to_16khz_mono, gemini_pcm_24khz_to_48khz_stereo};

    #[test]
    fn downsamples_a_stereo_48khz_frame_to_320_mono_samples() {
        let input = vec![1_i16; 1_920]
            .into_iter()
            .flat_map(i16::to_le_bytes)
            .collect::<Vec<_>>();
        let output = discord_pcm_to_16khz_mono(&input);
        assert_eq!(output.len(), 320 * 2);
    }

    #[test]
    fn upsamples_24khz_mono_to_48khz_stereo() {
        let input = vec![1_i16, 2, 3]
            .into_iter()
            .flat_map(i16::to_le_bytes)
            .collect::<Vec<_>>();
        let output = gemini_pcm_24khz_to_48khz_stereo(&input);
        assert_eq!(output.len(), 12);
        assert_eq!(output[0], 1);
        assert_eq!(output[1], 1);
        assert_eq!(output[4], 2);
        assert_eq!(output[5], 2);
    }
}
