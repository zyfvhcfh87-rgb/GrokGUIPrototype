import { useId, useState } from "react";
import type { InteractionController, InteractionStatus } from "../application/interaction-controller.ts";
import { elicitationAnswer, pendingInteractions, permissionActions, permissionConsequenceTone, responseKey, validElicitationControl, validPermissionScope, type FormInput, type Interaction, type InteractionContext } from "../application/interactions.ts";

export function InteractionPane({ context, controller, statuses }: {
  context: InteractionContext;
  controller: InteractionController;
  statuses: Record<string, InteractionStatus>;
}) {
  const requests = pendingInteractions(context);
  if (requests.length === 0) return null;
  return (
    <section className="interaction-list" aria-label="Pending requests">
      <p className="shell-panel__eyebrow" role="status">Your input is needed</p>
      {requests.map(target => <InteractionCard key={target.key} target={target} controller={controller} status={statuses[responseKey(target)]} />)}
    </section>
  );
}

function InteractionCard({ target, controller, status }: {
  target: Interaction; controller: InteractionController; status: InteractionStatus | undefined;
}) {
  const labelId = useId();
  if (target.kind === "elicitation") return <ElicitationForm target={target} controller={controller} status={status} />;
  const { request } = target;
  const scope = request.scope;
  const valid = validPermissionScope(scope);
  const actions = permissionActions(request);
  const tone = permissionConsequenceTone(request);
  return (
    <article className="stream-card interaction-card" aria-labelledby={labelId} aria-busy={status?.phase === "sending"}>
      <h2 id={labelId}>{request.title}</h2>
      <p className={`interaction-tone interaction-tone--${tone}`}>
        {tone === "destructive" ? "Destructive action. Review the exact command and paths before allowing it." : "Review required. Approval is never implied by color."}
      </p>
      <p>Review what this action can do before allowing it.</p>
      {valid && scope.type === "command" ? <dl className="interaction-scope">
        <dt>Command</dt><dd><pre>{scope.command}</pre></dd>
        <dt>Working directory</dt><dd><code>{scope.workingDirectory}</code></dd>
        <dt>Affected paths</dt><dd>{scope.affectedPaths.length > 0 ? <ul>{scope.affectedPaths.map(path => <li key={path}><code>{path}</code></li>)}</ul> : "Not supplied"}</dd>
        <dt>Requested scope</dt><dd>Execute this command in the directory shown.</dd>
      </dl> : valid && scope.type === "filesystem" ? <dl className="interaction-scope">
        <dt>Operation</dt><dd>{scope.operation}</dd><dt>Affected path</dt><dd><code>{scope.path}</code></dd>
      </dl> : <p role="alert">The requested scope is missing or ambiguous. Approval is unavailable.</p>}
      <p><strong>Consequence: </strong>{request.consequence ?? "No additional consequence was supplied."}</p>
      {actions.some(action => action.persistent) ? <p className="interaction-note">“Always” asks Grok to remember this decision. Its duration and matching rules are controlled by the runtime.</p> : null}
      <ResponseStatus status={status} />
      <div className="interaction-actions">
        {actions.map(action => <button className="button" key={action.decision} type="button" disabled={status !== undefined}
          onClick={() => void controller.respondPermission(target, action.decision)}>{action.label}</button>)}
      </div>
    </article>
  );
}

function ElicitationForm({ target, controller, status }: {
  target: Extract<Interaction, { kind: "elicitation" }>; controller: InteractionController; status: InteractionStatus | undefined;
}) {
  const id = useId();
  const [input, setInput] = useState<FormInput>(null);
  const [error, setError] = useState<string | null>(null);
  const control = target.request.control;
  const valid = validElicitationControl(control);
  const disabled = status !== undefined;
  const fieldLabel = control.type === "other" ? "Response" : control.label ?? "Response";
  const cancel = () => { setInput(null); setError(null); void controller.cancelElicitation(target); };
  return (
    <form className="stream-card interaction-card" aria-labelledby={`${id}-title`} aria-busy={status?.phase === "sending"}
      autoComplete="off" noValidate onSubmit={event => {
        event.preventDefault();
        // Submission is explicit: Enter in a field must never implicitly accept.
      }} onKeyDown={event => {
        if (event.key === "Escape") { event.preventDefault(); if (!disabled) cancel(); }
        if (event.key === "Enter" && event.target instanceof HTMLInputElement) event.preventDefault();
      }}>
      <h2 id={`${id}-title`}>{target.request.prompt}</h2>
      {!valid ? <p role="alert">This form cannot be answered safely. Cancel the request.</p> : <fieldset disabled={disabled}>
        <legend>{fieldLabel}</legend>
        {control.type === "text" ? <>
          <label htmlFor={id}>{fieldLabel}</label>
          <input id={id} type={control.sensitive ? "password" : "text"} value={typeof input === "string" ? input : ""}
            autoComplete="off" spellCheck={false} autoCapitalize="none" maxLength={control.maxLength * 2}
            aria-invalid={error !== null} aria-describedby={`${id}-hint${error ? ` ${id}-error` : ""}`}
            onChange={event => { setInput(event.target.value); setError(null); }} />
          <small id={`${id}-hint`}>{control.minLength}–{control.maxLength} characters, up to 16 KB. Sent only when you choose Accept.</small>
        </> : control.type === "confirmation" ? <div className="interaction-options">
          {([false, true] as const).map(value => <label key={String(value)}><input type="radio" name={id} checked={input === value}
            onChange={() => { setInput(value); setError(null); }} />{value ? "Yes" : "No"}</label>)}
        </div> : control.type === "choice" ? control.multiple ? <div className="interaction-options">
          {control.options.map(value => <label key={value}><input type="checkbox" checked={Array.isArray(input) && input.includes(value)}
            onChange={event => { const values = Array.isArray(input) ? input : []; setInput(event.target.checked ? [...values, value] : values.filter(item => item !== value)); setError(null); }} />{value}</label>)}
        </div> : <><label htmlFor={id}>{fieldLabel}</label><select id={id} value={typeof input === "string" ? input : ""}
          aria-invalid={error !== null} aria-describedby={error ? `${id}-error` : undefined}
          onChange={event => { setInput(event.target.value); setError(null); }}>
          <option value="" disabled>Choose an option</option>{control.options.map(value => <option key={value} value={value}>{value}</option>)}
        </select></> : null}
      </fieldset>}
      {error ? <p id={`${id}-error`} role="alert">{error}</p> : null}
      <ResponseStatus status={status} />
      <div className="interaction-actions">
        <button className="button" type="button" disabled={disabled} onClick={cancel}>Cancel request</button>
        {valid ? <button className="button button--primary" type="button" disabled={disabled} onClick={() => {
          const answer = elicitationAnswer(control, input ?? (control.type === "text" ? "" : null));
          if (answer.error !== null) { setError(answer.error); return; }
          void controller.respondElicitation(target, input ?? "");
          setInput(null);
          setError(null);
        }}>Accept</button> : null}
      </div>
    </form>
  );
}

function ResponseStatus({ status }: { status: InteractionStatus | undefined }) {
  if (status === undefined) return null;
  return <p role={status.phase === "failed" ? "alert" : "status"}>{status.error ?? (status.phase === "sending" ? "Sending response…" : "Response sent.")}</p>;
}
