import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import { APP_IDENTITY } from "../src/application/compatibility.ts";

const readProjectFile = async (relativePath) =>
  readFile(new URL(`../${relativePath}`, import.meta.url), "utf8");

const readJson = async (relativePath) => JSON.parse(await readProjectFile(relativePath));

test("Windows-first packaging does not bundle Grok, an updater, or signing secrets", async () => {
  const config = await readJson("src-tauri/tauri.conf.json");
  const packageManifest = await readJson("package.json");

  assert.deepEqual(config.bundle.targets, ["nsis"]);
  assert.equal(config.bundle.createUpdaterArtifacts, false);
  assert.deepEqual(config.bundle.resources, []);
  assert.deepEqual(config.bundle.externalBin, []);
  assert.equal(config.bundle.windows.certificateThumbprint, null);
  assert.equal(config.bundle.windows.timestampUrl, "");
  assert.equal(config.plugins && Object.keys(config.plugins).length, 0);
  assert.equal(packageManifest.version, APP_IDENTITY.version);
  assert.equal(config.version, APP_IDENTITY.version);
  assert.doesNotMatch(JSON.stringify(config), /grok\.exe|auth\.json|TAURI_SIGNING_PRIVATE_KEY/u);
});

test("the repository does not store signing credentials", async () => {
  const gitignore = await readProjectFile(".gitignore");
  assert.match(gitignore, /\.env/u);
  const workflow = await readProjectFile(".github/workflows/windows-qa.yml");
  assert.doesNotMatch(workflow, /TAURI_SIGNING_PRIVATE_KEY/u);
  assert.doesNotMatch(workflow, /certificateThumbprint:\s*["'][A-Fa-f0-9]/u);
  assert.match(workflow, /Name -eq 'grok\.exe'/u);
  assert.match(workflow, /Filter grok\.exe/u);
});