import assert from "node:assert/strict";
import test from "node:test";

import { validateExternalUrl } from "../src/application/urls.ts";

test("http and https URLs without credentials are accepted", () => {
  assert.equal(validateExternalUrl("https://example.com/docs").ok, true);
  assert.equal(validateExternalUrl("http://example.com").ok, true);
  assert.equal(validateExternalUrl(" HTTPS://example.com/a ").ok, true);
});

test("unsafe schemes, credentials, and junk are rejected", () => {
  assert.equal(validateExternalUrl("javascript:alert(1)").reason, "unsupported_scheme");
  assert.equal(validateExternalUrl("data:text/html,hi").reason, "unsupported_scheme");
  assert.equal(validateExternalUrl("file:///etc/passwd").reason, "unsupported_scheme");
  assert.equal(validateExternalUrl("https://user:secret@example.com").reason, "credentials");
  assert.equal(validateExternalUrl("https://example.com/has space").reason, "invalid");
  assert.equal(validateExternalUrl("").reason, "empty");
  assert.equal(validateExternalUrl("not a url").reason, "invalid");
});
