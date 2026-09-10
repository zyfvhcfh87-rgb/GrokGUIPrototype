import type { AvailableCommand } from "./contract.ts";

export type PlanReviewAction = "approve" | "revise";

export type AdvertisedPlanReview = {
  approve: AvailableCommand | null;
  revise: AvailableCommand | null;
};

const APPROVE_COMMANDS = new Set([
  "approve_plan",
  "approve-plan",
  "plan_approve",
  "plan-approve",
]);

const REVISE_COMMANDS = new Set([
  "revise_plan",
  "revise-plan",
  "plan_revise",
  "plan-revise",
]);

export function advertisedPlanReview(commands: readonly AvailableCommand[]): AdvertisedPlanReview {
  return {
    approve: commands.find((command) => APPROVE_COMMANDS.has(command.name)) ?? null,
    revise: commands.find((command) => REVISE_COMMANDS.has(command.name)) ?? null,
  };
}

export function planReviewCommand(
  commands: readonly AvailableCommand[],
  action: PlanReviewAction,
): AvailableCommand | null {
  return advertisedPlanReview(commands)[action];
}

export function formatPlanCommand(command: AvailableCommand): string {
  const name = command.name.startsWith("/") ? command.name : `/${command.name}`;
  return command.acceptsInput ? `${name} ` : name;
}
