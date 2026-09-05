import type { ConfigOption, ConfigValue } from "../application/contract.ts";
import type { ConversationPresentation } from "../application/conversation.ts";
import { describeActivityStatus, describeToolKind } from "../application/conversation.ts";

export function ActivityPane({
  presentation,
  controlBusy,
  onModelChange,
  onReasoningChange,
  onModeChange,
  onConfigChange,
  onInsertCommand,
}: {
  presentation: ConversationPresentation;
  controlBusy: boolean;
  onModelChange: (modelId: string) => void;
  onReasoningChange: (value: string) => void;
  onModeChange: (modeId: string) => void;
  onConfigChange: (configId: string, value: ConfigValue) => void;
  onInsertCommand: (name: string, acceptsInput: boolean) => void;
}) {
  const { controls, activity } = presentation;
  const usage = activity.usage;

  return (
    <aside className="activity-pane" aria-labelledby="activity-heading">
      <p className="shell-panel__eyebrow">Activity</p>
      <h1 id="activity-heading">Plan and controls</h1>

      {controls.canChangeModel ||
      controls.canChangeReasoning ||
      controls.canChangeMode ||
      controls.canChangeConfig ? (
        <div className="control-stack" aria-label="Negotiated session controls">
          {controls.canChangeModel ? (
            <label>
              <span>Model</span>
              <select
                value={controls.currentModelId ?? ""}
                disabled={controlBusy}
                onChange={(event) => onModelChange(event.target.value)}
              >
                {controls.models.map((model) => (
                  <option key={model.id} value={model.id}>
                    {model.label}
                  </option>
                ))}
              </select>
            </label>
          ) : null}
          {controls.canChangeReasoning ? (
            <label>
              <span>Reasoning</span>
              <select
                value={controls.currentReasoningValue ?? ""}
                disabled={controlBusy}
                onChange={(event) => onReasoningChange(event.target.value)}
              >
                {controls.reasoning.map((effort) => (
                  <option key={effort.id} value={effort.value}>
                    {effort.label}
                  </option>
                ))}
              </select>
            </label>
          ) : null}
          {controls.canChangeMode ? (
            <label>
              <span>Mode</span>
              <select
                value={controls.currentModeId ?? ""}
                disabled={controlBusy}
                onChange={(event) => onModeChange(event.target.value)}
              >
                {controls.modes.map((mode) => (
                  <option key={mode.id} value={mode.id}>
                    {mode.label}
                  </option>
                ))}
              </select>
            </label>
          ) : null}
          {controls.configOptions.map((option) => (
            <ConfigControl
              key={option.id}
              option={option}
              disabled={controlBusy}
              onChange={onConfigChange}
            />
          ))}
        </div>
      ) : (
        <p className="activity-pane__muted">No optional controls were advertised.</p>
      )}

      {controls.commands.length > 0 ? (
        <div className="command-list" aria-label="Advertised commands">
          {controls.commands.map((command) => (
            <button
              key={command.name}
              className="chip-button"
              type="button"
              title={command.description}
              onClick={() => onInsertCommand(command.name, command.acceptsInput)}
            >
              /{command.name}
            </button>
          ))}
        </div>
      ) : null}

      {usage !== null ? (
        <article className="stream-card stream-card--usage">
          <header>
            <strong>Usage</strong>
          </header>
          <dl className="usage-list">
            {usage.inputTokens !== null ? (
              <div>
                <dt>Input</dt>
                <dd>{usage.inputTokens}</dd>
              </div>
            ) : null}
            {usage.outputTokens !== null ? (
              <div>
                <dt>Output</dt>
                <dd>{usage.outputTokens}</dd>
              </div>
            ) : null}
            {usage.cachedInputTokens !== null ? (
              <div>
                <dt>Cached</dt>
                <dd>{usage.cachedInputTokens}</dd>
              </div>
            ) : null}
            {usage.totalTokens !== null ? (
              <div>
                <dt>Total</dt>
                <dd>{usage.totalTokens}</dd>
              </div>
            ) : null}
            {usage.contextWindowTokens !== null ? (
              <div>
                <dt>Window</dt>
                <dd>{usage.contextWindowTokens}</dd>
              </div>
            ) : null}
          </dl>
        </article>
      ) : null}

      {activity.plan.length > 0 ? (
        <article className="stream-card">
          <header>
            <strong>Plan</strong>
          </header>
          <ol className="plan-list">
            {activity.plan.map((entry) => (
              <li key={entry.id}>
                <strong>{entry.title}</strong>
                <small>{entry.status.replaceAll("_", " ")}</small>
                {entry.description !== null ? <span>{entry.description}</span> : null}
              </li>
            ))}
          </ol>
        </article>
      ) : null}

      {activity.tools.length > 0 ? (
        <ul className="activity-tools" aria-label="Tool activity">
          {activity.tools.map((tool) => (
            <li key={tool.id}>
              <strong>{tool.title}</strong>
              <small>
                {describeToolKind(tool.kind)} · {describeActivityStatus(tool.status)}
              </small>
            </li>
          ))}
        </ul>
      ) : null}
    </aside>
  );
}

function ConfigControl({
  option,
  disabled,
  onChange,
}: {
  option: ConfigOption;
  disabled: boolean;
  onChange: (configId: string, value: ConfigValue) => void;
}) {
  if (option.kind.type === "boolean") {
    return (
      <label className="control-toggle">
        <input
          type="checkbox"
          checked={option.kind.currentValue}
          disabled={disabled}
          onChange={(event) =>
            onChange(option.id, { type: "boolean", value: event.target.checked })
          }
        />
        <span>{option.name}</span>
      </label>
    );
  }
  return (
    <label>
      <span>{option.name}</span>
      <select
        value={option.kind.currentValue}
        disabled={disabled}
        onChange={(event) =>
          onChange(option.id, { type: "select", value: event.target.value })
        }
      >
        {option.kind.options.map((choice) => (
          <option key={choice.value} value={choice.value}>
            {choice.name}
          </option>
        ))}
      </select>
    </label>
  );
}
