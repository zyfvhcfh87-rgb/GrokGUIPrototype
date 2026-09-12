import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const readProjectFile = async (relativePath) =>
  readFile(new URL(`../${relativePath}`, import.meta.url), "utf8");

const readJson = async (relativePath) =>
  JSON.parse(await readProjectFile(relativePath));

test("the desktop window loads only local production assets", async () => {
  const config = await readJson("src-tauri/tauri.conf.json");
  const [mainWindow] = config.app.windows;

  assert.equal(config.build.frontendDist, "../dist");
  assert.equal(config.build.devUrl, "http://127.0.0.1:1420");
  assert.deepEqual(config.app.windows.map(({ label }) => label), ["main"]);
  assert.equal(mainWindow.url, "index.html");
  assert.equal(config.app.withGlobalTauri, false);
  assert.deepEqual(config.plugins, {});
});

test("the main capability grants only event subscription permissions", async () => {
  const capability = await readJson("src-tauri/capabilities/main-shell.json");

  assert.equal(capability.local, true);
  assert.deepEqual(capability.windows, ["main"]);
  assert.deepEqual(capability.permissions, [
    "core:event:allow-listen",
    "core:event:allow-unlisten",
  ]);
  assert.equal(Object.hasOwn(capability, "remote"), false);
});

test("production CSP is local-only and blocks embeddable active content", async () => {
  const config = await readJson("src-tauri/tauri.conf.json");
  const { csp, devCsp } = config.app.security;

  assert.equal(csp["default-src"], "'self'");
  assert.equal(csp["object-src"], "'none'");
  assert.equal(csp["frame-src"], "'none'");
  assert.equal(csp["base-uri"], "'none'");
  assert.equal(csp["form-action"], "'none'");
  assert.equal(csp["connect-src"], "'self' ipc: http://ipc.localhost");
  assert.doesNotMatch(JSON.stringify(csp), /https:|wss?:/u);
  assert.equal(
    devCsp["connect-src"],
    "'self' ipc: http://ipc.localhost ws://127.0.0.1:1420",
  );
  assert.equal(config.app.security.freezePrototype, true);
  assert.equal(config.app.security.assetProtocol.enable, false);
  assert.equal(config.app.security.dangerousDisableAssetCspModification, false);
});

test("the scaffold does not include native access plugins", async () => {
  const packageManifest = await readProjectFile("package.json");
  const cargoManifest = await readProjectFile("src-tauri/Cargo.toml");
  const manifests = `${packageManifest}\n${cargoManifest}`;

  assert.doesNotMatch(
    manifests,
    /(?:@tauri-apps\/plugin-|tauri-plugin-)(?:dialog|fs|http|opener|os|process|shell|upload|websocket)/u,
  );
});

test("the desktop bridge exposes only reviewed application commands and events", async () => {
  const bridge = await readProjectFile("src-tauri/src/lib.rs");
  const contract = await readProjectFile(
    "src-tauri/src/application_contract.rs",
  );
  const productionContract = contract.split("#[cfg(test)]", 1)[0];

  for (const command of [
    "setup_status",
    "workspace_pick",
    "workspace_validate",
    "workspace_recent_list",
    "workspace_recent_remove",
    "workspace_changes",
    "open_external_url",
    "runtime_snapshot",
    "runtime_diagnostics",
    "runtime_start",
    "runtime_stop",
    "runtime_restart",
    "session_new",
    "session_list",
    "session_load",
    "session_resume",
    "session_close",
    "prompt_send",
    "prompt_cancel",
    "session_set_mode",
    "session_set_model",
    "session_set_config",
    "permission_respond",
    "elicitation_respond",
  ]) {
    assert.match(bridge, new RegExp(`(?:async )?fn ${command}\\b`, "u"));
  }
  assert.match(contract, /pub enum ApplicationEvent/u);
  assert.match(contract, /pub struct ApplicationEventEnvelope/u);
  assert.match(contract, /pub struct ApplicationEventClock/u);
  assert.match(bridge, /\.manage\(manager\)/u);
  assert.match(bridge, /\.invoke_handler\(tauri::generate_handler!/u);
  assert.match(bridge, /emit\(APPLICATION_EVENT_NAME, envelope\)/u);
  assert.match(bridge, /RecvError::Lagged/u);
  assert.doesNotMatch(bridge, /async fn runtime_execute\b/u);
  assert.doesNotMatch(contract, /pub (?:enum|struct) (?:RuntimeCommand|RuntimeResponse)/u);
  assert.doesNotMatch(bridge, /session\/(?:new|list|load|resume|prompt|cancel|close|set_)/u);
  assert.doesNotMatch(
    `${bridge}\n${productionContract}`,
    /serde_json::Value|JsonRpc|request_id|grok agent stdio|auth\.json/u,
  );
  assert.doesNotMatch(
    `${bridge}\n${productionContract}`,
    /stderr_text|stderr_output|ChildStderr/u,
  );
});

test("the development server does not watch Rust build outputs", async () => {
  const viteConfig = await readProjectFile("vite.config.ts");

  assert.match(viteConfig, /\*\*\/src-tauri\/\*\*/u);
  assert.match(viteConfig, /\*\*\/target\/\*\*/u);
});
