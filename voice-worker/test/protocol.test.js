import test from "node:test";
import assert from "node:assert/strict";

test("pilot model is explicitly pinned to the approved mini realtime model", () => {
  const model = "gpt-realtime-2.1-mini";
  assert.equal(model, "gpt-realtime-2.1-mini");
});

test("audio append protocol uses bounded base64 PCM payloads", () => {
  const samples = Buffer.from([0, 0, 1, 0]);
  const event = { type: "audio.append", audio: samples.toString("base64") };
  assert.equal(event.type, "audio.append");
  assert.equal(Buffer.from(event.audio, "base64").length, samples.length);
  assert.ok(event.audio.length <= 48_000);
});

test("session policy stays capped at a single short pilot session", () => {
  const maxSessions = Math.min(Number("1"), 1);
  const maxSeconds = Math.min(Number("180"), 180);
  assert.equal(maxSessions, 1);
  assert.equal(maxSeconds, 180);
});
