import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import type { ApplicationTransport } from "./bridge.ts";

export function createTauriTransport(): ApplicationTransport {
  return {
    invoke: (command, args) => invoke(command, args),
    listen: (eventName, listener) => listen(eventName, listener),
  };
}
