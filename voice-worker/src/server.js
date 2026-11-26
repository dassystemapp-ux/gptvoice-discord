import http from "node:http";
import { timingSafeEqual } from "node:crypto";
import { WebSocket, WebSocketServer } from "ws";

const HOST = process.env.HOST ?? "127.0.0.1";
const PORT = Number(process.env.PORT ?? 8787);
const MODEL = process.env.DAS_REALTIME_MODEL ?? "gpt-realtime-2.1-mini";
const VOICE = process.env.DAS_REALTIME_VOICE ?? "marin";
const MAX_SESSION_SECONDS = Math.min(Number(process.env.DAS_MAX_SESSION_SECONDS ?? 180), 180);
const MAX_SESSIONS = Math.min(Number(process.env.DAS_MAX_SESSIONS ?? 1), 1);
const API_KEY = process.env.OPENAI_API_KEY;
const WORKER_TOKEN = process.env.DAS_WORKER_TOKEN;

function safeEqual(a, b) {
  if (typeof a !== "string" || typeof b !== "string") return false;
  const aa = Buffer.from(a);
  const bb = Buffer.from(b);
  return aa.length === bb.length && timingSafeEqual(aa, bb);
}

function instructions() {
  return [
    "أنت مساعد DAS الصوتي، اسمك الرحّال.",
    "تحدث بعربية سعودية طبيعية وواضحة، بنبرة هادئة وودودة وغير متكلفة.",
    "أجب بإيجاز مفيد. لا تلقِ محاضرة إلا إذا طُلب منك التفصيل.",
    "اسأل سؤال توضيح واحدًا عند الحاجة. لا تتكلم فوق المستخدم.",
    "إذا قاطعك المستخدم فتوقف فورًا واستمع إليه.",
    "لا تدّع أنك نفذت إجراءً أو فتحت ملفًا أو غيّرت إعدادًا ما لم يحدث ذلك.",
    "احترم خصوصية الأعضاء ولا تكشف بياناتهم لبعضهم."
  ].join("\n");
}

const server = http.createServer((req, res) => {
  if (req.url === "/health" && req.method === "GET") {
    res.writeHead(200, { "content-type": "application/json" });
    res.end(JSON.stringify({ ok: true, model: MODEL, activeSessions: sessions.size }));
    return;
  }
  res.writeHead(404);
  res.end();
});

const wss = new WebSocketServer({ noServer: true, maxPayload: 64 * 1024 });
const sessions = new Set();

server.on("upgrade", (req, socket, head) => {
  const remote = req.socket.remoteAddress ?? "";
  const localOnly = remote === "127.0.0.1" || remote === "::1" || remote === "::ffff:127.0.0.1";
  if (!localOnly || !WORKER_TOKEN || !safeEqual(req.headers["x-das-worker-token"], WORKER_TOKEN)) {
    socket.write("HTTP/1.1 401 Unauthorized\r\n\r\n");
    socket.destroy();
    return;
  }
  if (!API_KEY) {
    socket.write("HTTP/1.1 503 Service Unavailable\r\n\r\n");
    socket.destroy();
    return;
  }
  if (sessions.size >= MAX_SESSIONS) {
    socket.write("HTTP/1.1 429 Too Many Requests\r\n\r\n");
    socket.destroy();
    return;
  }
  wss.handleUpgrade(req, socket, head, ws => wss.emit("connection", ws, req));
});

wss.on("connection", client => {
  const session = { client, upstream: null, startedAt: Date.now(), closed: false, ready: false };
  sessions.add(session);

  const close = () => {
    if (session.closed) return;
    session.closed = true;
    clearTimeout(timer);
    sessions.delete(session);
    try { session.upstream?.close(); } catch {}
    try { client.close(); } catch {}
  };

  const timer = setTimeout(() => {
    client.send(JSON.stringify({ type: "session.limit_reached" }));
    close();
  }, MAX_SESSION_SECONDS * 1000);

  const upstream = new WebSocket(
    `wss://api.openai.com/v1/realtime?model=${encodeURIComponent(MODEL)}`,
    { headers: { Authorization: `Bearer ${API_KEY}` } }
  );
  session.upstream = upstream;

  upstream.on("open", () => {
    upstream.send(JSON.stringify({
      type: "session.update",
      session: {
        type: "realtime",
        model: MODEL,
        output_modalities: ["audio"],
        instructions: instructions(),
        audio: {
          input: {
            format: { type: "audio/pcm", rate: 24000 },
            noise_reduction: { type: "near_field" },
            turn_detection: { type: "server_vad", threshold: 0.55, prefix_padding_ms: 240, silence_duration_ms: 900, interrupt_response: true, create_response: true }
          },
          output: { format: { type: "audio/pcm", rate: 24000 }, voice: VOICE }
        },
        max_output_tokens: 180
      }
    }));
  });

  upstream.on("message", data => {
    const raw = data.toString();
    let event;
    try { event = JSON.parse(raw); } catch { return; }
    if (event.type === "session.updated") session.ready = true;
    if (client.readyState === WebSocket.OPEN) client.send(raw);
  });

  upstream.on("error", () => {
    if (client.readyState === WebSocket.OPEN) client.send(JSON.stringify({ type: "worker.error", message: "Realtime connection failed" }));
    close();
  });
  upstream.on("close", () => close());

  client.on("message", data => {
    if (!session.ready || upstream.readyState !== WebSocket.OPEN) return;
    let event;
    try { event = JSON.parse(data.toString()); } catch { return; }
    if (event.type === "audio.append" && typeof event.audio === "string" && event.audio.length <= 48000) {
      upstream.send(JSON.stringify({ type: "input_audio_buffer.append", audio: event.audio }));
    } else if (event.type === "response.cancel") {
      upstream.send(JSON.stringify({ type: "response.cancel" }));
    } else if (event.type === "session.close") {
      clearTimeout(timer);
      close();
    }
  });

  client.on("close", () => { clearTimeout(timer); close(); });
  client.on("error", () => { clearTimeout(timer); close(); });
});

server.listen(PORT, HOST, () => {
  console.log(`DAS voice worker listening on http://${HOST}:${PORT}`);
});

process.on("SIGINT", () => { for (const s of sessions) { try { s.upstream?.close(); s.client.close(); } catch {} } server.close(); });
process.on("SIGTERM", () => { for (const s of sessions) { try { s.upstream?.close(); s.client.close(); } catch {} } server.close(); });
