import type { ApplicationBridge } from "./bridge.ts";
import type { PermissionDecision } from "./contract.ts";
import { elicitationAnswer, pendingInteractions, permissionActions, responseKey, type FormInput, type Interaction, type InteractionContext } from "./interactions.ts";

type Bridge = Pick<ApplicationBridge, "respondToPermission" | "respondToElicitation">;
export type InteractionStatus = { phase: "sending" | "submitted" | "failed"; error: string | null };

/** Keeps only response status. Form values never enter the application store. */
export function createInteractionController(bridge: Bridge, readContext: () => InteractionContext) {
  let statuses: Record<string, InteractionStatus> = {};
  const listeners = new Set<() => void>();
  const publish = (key: string, status: InteractionStatus) => {
    statuses = { ...statuses, [key]: status };
    for (const listener of listeners) listener();
  };
  const isCurrent = (target: Interaction) => pendingInteractions(readContext()).some(item =>
    item.key === target.key && item.request === target.request);
  const send = async (target: Interaction, invoke: () => ReturnType<Bridge["respondToPermission"]>) => {
    const key = responseKey(target);
    if (!isCurrent(target) || statuses[key] !== undefined) return false;
    // Claim synchronously before yielding so double clicks cannot send twice.
    publish(key, { phase: "sending", error: null });
    try {
      const acknowledgement = await invoke();
      if (!acknowledgement.acknowledged) throw new Error("Response not accepted");
      if (statuses[key] !== undefined) publish(key, { phase: "submitted", error: null });
      return true;
    } catch {
      // Never display a transport error that might echo sensitive form content.
      if (statuses[key] !== undefined) publish(key, {
        phase: "failed", error: "The response could not be confirmed. Cancel the turn or reconnect before trying again.",
      });
      return false;
    }
  };
  return {
    getState: () => statuses,
    subscribe: (listener: () => void) => { listeners.add(listener); return () => { listeners.delete(listener); }; },
    reconcile: () => {
      // Temporary UI blocking or navigation must not release a submission lock.
      const { application } = readContext();
      const current = new Set<string>();
      const remember = (kind: string, id: string) => current.add(JSON.stringify([application.generation, kind, id]));
      Object.keys(application.runtime.elicitations).forEach(id => remember("elicitation", id));
      for (const session of Object.values(application.sessions)) {
        Object.keys(session.permissions).forEach(id => remember("permission", id));
        Object.keys(session.elicitations).forEach(id => remember("elicitation", id));
      }
      const entries = Object.entries(statuses).filter(([key]) => current.has(key));
      if (entries.length !== Object.keys(statuses).length) {
        statuses = Object.fromEntries(entries);
        for (const listener of listeners) listener();
      }
    },
    respondPermission: (target: Interaction, decision: PermissionDecision) => {
      if (target.kind !== "permission" || !permissionActions(target.request).some(action => action.decision === decision)) return Promise.resolve(false);
      return send(target, () => bridge.respondToPermission({ interactionId: target.request.id, decision }));
    },
    respondElicitation: (target: Interaction, input: FormInput) => {
      if (target.kind !== "elicitation") return Promise.resolve(false);
      const result = elicitationAnswer(target.request.control, input);
      const decision = result.decision;
      if (decision === null) return Promise.resolve(false);
      return send(target, () => bridge.respondToElicitation({ interactionId: target.request.id, decision }));
    },
    cancelElicitation: (target: Interaction) => target.kind !== "elicitation" ? Promise.resolve(false) :
      send(target, () => bridge.respondToElicitation({ interactionId: target.request.id, decision: { type: "cancel" } })),
  } as const;
}

export type InteractionController = ReturnType<typeof createInteractionController>;
