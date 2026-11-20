import assert from "node:assert/strict";
import { test } from "node:test";
import { parseLocalServers } from "./localServers.ts";

test("parses the detect-local JSON and drops anything unknown or malformed", () => {
  const json = JSON.stringify([
    { id: "ollama", base_url: "http://127.0.0.1:11434/v1", models: ["llama3.2:3b", 7] },
    { id: "evil", base_url: "http://example.com", models: [] },
    { id: "lmstudio", base_url: "http://127.0.0.1:1234/v1" }
  ]);
  assert.deepEqual(parseLocalServers(json), [
    { id: "ollama", baseUrl: "http://127.0.0.1:11434/v1", models: ["llama3.2:3b"] }
  ]);
  assert.deepEqual(parseLocalServers("not json"), []);
  assert.deepEqual(parseLocalServers("{}"), []);
});
