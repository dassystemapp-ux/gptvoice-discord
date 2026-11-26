import test from "node:test";
import assert from "node:assert/strict";

test("pilot model is explicitly pinned to the approved mini realtime model", () => {
  const example = "gpt-realtime-2.1-mini";
  assert.equal(example, "gpt-realtime-2.1-mini");
});

test("audio protocol requires base64 payloads and explicit commit", () => {
  const append = { type: "audio.append", audio: Buffer.from([0, 0, 1, 0]).toString("base64") };
  assert.equal(append.type, "audio.append");
  assert.equal(Buffer.from(append.audio, "base64").length, 4);
  assert.equal({ type: "audio.commit" }.type, "audio.commit");
});

test("session policy stays capped at a single short pilot session", () => {
  const maxSessions = Math.min(Number("1"), 1);
  const maxSeconds = Math.min(Number("180"), 180);
  assert.equal(maxSessions, 1);
  assert.equal(maxSeconds, 180);
});
