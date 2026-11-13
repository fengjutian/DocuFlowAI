import test from "node:test";
import assert from "node:assert/strict";
import { extractPromptText } from "../../acp/server.js";

/**
 * The prompt's blocks.
 *
 * The instruction and the attached material travel together but stay apart:
 * the model has to know which is which, otherwise it cannot tell a request
 * from a file.
 */
test("acp: the instruction and the context travel as separate blocks", () => {
  const text = extractPromptText({
    sessionId: "sess-1",
    prompt: [
      { type: "text", text: "make this function async" },
      { type: "resource", uri: "src/app.rs", text: "fn main() {}" }
    ]
  });

  assert.match(text, /make this function async/);
  assert.match(text, /src\/app\.rs/);
  assert.match(text, /fn main\(\) \{\}/);
});

test("acp: the path of a context block stays with its text", () => {
  const text = extractPromptText({
    sessionId: "sess-1",
    prompt: [{ type: "resource", uri: "src/other.rs", text: "content" }]
  });

  // The model has to know which file the material came from; a text block
  // without its path is just text.
  assert.match(text, /src\/other\.rs/);
  assert.match(text, /content/);
});

test("acp: several context blocks all arrive", () => {
  const text = extractPromptText({
    sessionId: "sess-1",
    prompt: [
      { type: "text", text: "compare these" },
      { type: "resource", uri: "a.rs", text: "first" },
      { type: "resource", uri: "b.rs", text: "second" }
    ]
  });

  assert.match(text, /a\.rs/);
  assert.match(text, /first/);
  assert.match(text, /b\.rs/);
  assert.match(text, /second/);
});

test("acp: a prompt with no blocks is empty", () => {
  assert.equal(extractPromptText({ sessionId: "sess-1", prompt: [] }), "");
});
