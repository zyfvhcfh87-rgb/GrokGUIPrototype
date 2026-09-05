import assert from "node:assert/strict";
import test from "node:test";

import { parseInlines, parseMarkdown } from "../src/application/markdown.ts";

test("markdown keeps a safe subset and leaves HTML as text", () => {
  const blocks = parseMarkdown(`# Title

Paragraph with **bold** and \`code\`.

- one
- two

\`\`\`ts
const x = 1;
\`\`\`

<script>alert(1)</script>
`);

  assert.equal(blocks[0].type, "heading");
  assert.equal(blocks[1].type, "paragraph");
  assert.equal(blocks[2].type, "list");
  assert.equal(blocks[3].type, "code_block");
  assert.equal(blocks[3].value, "const x = 1;");
  assert.equal(blocks[4].type, "paragraph");
  assert.equal(blocks[4].children[0].value, "<script>alert(1)</script>");
});

test("links are kept only after external URL validation", () => {
  const safe = parseInlines("[docs](https://example.com/docs)");
  assert.deepEqual(safe, [
    {
      type: "link",
      href: "https://example.com/docs",
      children: [{ type: "text", value: "docs" }],
    },
  ]);

  const unsafe = parseInlines("[xss](javascript:alert(1))");
  assert.deepEqual(unsafe, [{ type: "text", value: "xss" }]);

  const image = parseInlines("![alt](https://example.com/x.png)");
  assert.equal(image[0].type, "link");
  assert.equal(image[0].children[0].value, "alt");
});
