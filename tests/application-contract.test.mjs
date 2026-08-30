import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import {
  APPLICATION_COMMANDS,
  APPLICATION_DTO_FIELDS,
  APPLICATION_ENUM_VALUES,
  APPLICATION_EVENT_FIELDS,
  APPLICATION_EVENT_NAME,
  APPLICATION_VARIANT_FIELDS,
} from "../src/application/contract.ts";

test("TypeScript command and event shapes match the shared contract manifest", async () => {
  const manifest = JSON.parse(
    await readFile(
      new URL("../fixtures/application-contract-manifest.json", import.meta.url),
      "utf8",
    ),
  );

  assert.equal(manifest.version, 1);
  assert.equal(APPLICATION_EVENT_NAME, manifest.eventName);
  assert.deepEqual(Object.values(APPLICATION_COMMANDS), manifest.commands);
  assert.deepEqual(APPLICATION_DTO_FIELDS, manifest.dtoFields);
  assert.deepEqual(APPLICATION_VARIANT_FIELDS, manifest.variantFields);
  assert.deepEqual(APPLICATION_ENUM_VALUES, manifest.enumValues);
  assert.deepEqual(APPLICATION_EVENT_FIELDS, manifest.events);
});
