import {
  nextOnboardingStep,
  previousOnboardingStep,
  type OnboardingPresentation,
  type OnboardingStepId,
} from "../application/onboarding.ts";
import type { RecoveryGuidance } from "../application/setup.ts";
import { Dialog } from "./Dialog.tsx";

export function OnboardingDialog({
  presentation,
  recovery,
  onStep,
  onSkip,
  onFinish,
  onPickWorkspace,
  onRetry,
}: {
  presentation: OnboardingPresentation;
  recovery: RecoveryGuidance | null;
  onStep: (step: OnboardingStepId) => void;
  onSkip: () => void;
  onFinish: () => void;
  onPickWorkspace: () => void;
  onRetry: () => void;
}) {
  const current = presentation.steps.find((step) => step.id === presentation.current) ??
    presentation.steps[0];

  return (
    <Dialog
      title="Setup guide"
      onClose={onSkip}
      footer={
        <>
          <button className="button" type="button" onClick={onSkip}>
            Skip guide
          </button>
          <button
            className="button"
            type="button"
            disabled={presentation.current === "welcome"}
            onClick={() => onStep(previousOnboardingStep(presentation.current))}
          >
            Back
          </button>
          {presentation.current === "compatibility" ? (
            <button className="button button--primary" type="button" onClick={onFinish}>
              {presentation.finishLabel}
            </button>
          ) : (
            <button
              className="button button--primary"
              type="button"
              disabled={!presentation.canContinue}
              onClick={() => onStep(nextOnboardingStep(presentation.current))}
            >
              Continue
            </button>
          )}
        </>
      }
    >
      <ol className="onboarding-steps">
        {presentation.steps.map((step) => (
          <li key={step.id} className={`onboarding-steps__item is-${step.status}`}>
            <button
              type="button"
              aria-current={step.id === presentation.current ? "step" : undefined}
              onClick={() => onStep(step.id)}
            >
              {step.title}
            </button>
          </li>
        ))}
      </ol>
      <h3>{current?.title}</h3>
      <p>{current?.detail}</p>
      {recovery !== null ? (
        <div className="inline-alert" role="status">
          <strong>{recovery.title}</strong>
          <span>{recovery.detail}</span>
        </div>
      ) : null}
      {presentation.current === "detect" || presentation.current === "authenticate" ? (
        <button className="button" type="button" onClick={onRetry}>
          Reconnect
        </button>
      ) : null}
      {presentation.current === "workspace" ? (
        <button className="button button--primary" type="button" onClick={onPickWorkspace}>
          Choose a workspace
        </button>
      ) : null}
      <p className="privacy-note">
        Skip or reopen this guide at any time. It does not start, stop, or authenticate Grok on
        its own.
      </p>
    </Dialog>
  );
}
