import type { FormEvent, KeyboardEvent } from "react";

import type {
  ConversationCard,
  ConversationPresentation,
} from "../application/conversation.ts";
import { MarkdownView } from "./MarkdownView.tsx";

export function ConversationPane({
  presentation,
  draft,
  onDraftChange,
  onSend,
  onCancel,
  onOpenUrl,
}: {
  presentation: ConversationPresentation;
  draft: string;
  onDraftChange: (value: string) => void;
  onSend: () => void;
  onCancel: () => void;
  onOpenUrl: (href: string) => void;
}) {
  const { composer } = presentation;

  const submit = (event: FormEvent) => {
    event.preventDefault();
    if (composer.canSend) {
      onSend();
    }
  };

  const onComposerKey = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      if (composer.canSend) {
        onSend();
      }
    }
  };

  return (
    <section className="conversation" aria-labelledby="conversation-heading">
      <header className="conversation__header">
        <p className="shell-panel__eyebrow">Conversation</p>
        <h1 id="conversation-heading">{presentation.heading}</h1>
        <p>{presentation.detail}</p>
      </header>

      <div className="conversation__transcript" aria-live="polite">
        {presentation.kind === "empty" || presentation.kind === "no_session" ? (
          <div className="empty-list">
            <span className="empty-list__icon" aria-hidden="true">
              ⌁
            </span>
            <p>{presentation.heading}</p>
            <span>{presentation.detail}</span>
          </div>
        ) : (
          presentation.cards.map((card) => (
            <ConversationCardView key={card.id} card={card} onOpenUrl={onOpenUrl} />
          ))
        )}
      </div>

      <form className="composer" onSubmit={submit}>
        <div className="composer__meta">
          <span className={`composer__state composer__state--${composer.kind}`}>
            {composer.label}
          </span>
          <span>{composer.detail}</span>
        </div>
        <label className="composer__label" htmlFor="conversation-composer">
          Prompt
        </label>
        <textarea
          id="conversation-composer"
          value={draft}
          disabled={!composer.draftEnabled}
          placeholder={
            composer.draftEnabled ? "Write a prompt. Enter sends, Shift+Enter adds a line." : ""
          }
          onChange={(event) => onDraftChange(event.target.value)}
          onKeyDown={onComposerKey}
        />
        <div className="composer__actions">
          {composer.canCancel ? (
            <button className="button" type="button" onClick={onCancel}>
              Cancel
            </button>
          ) : null}
          <button className="button button--primary" type="submit" disabled={!composer.canSend}>
            Send
          </button>
        </div>
      </form>
    </section>
  );
}

function ConversationCardView({
  card,
  onOpenUrl,
}: {
  card: ConversationCard;
  onOpenUrl: (href: string) => void;
}) {
  switch (card.type) {
    case "user_message":
      return (
        <article className="stream-card stream-card--user">
          <header>
            <strong>You</strong>
            {card.truncated ? <span>Truncated</span> : null}
          </header>
          <p className="stream-card__plain">{card.text}</p>
        </article>
      );
    case "assistant_message":
      return (
        <article className="stream-card stream-card--assistant">
          <header>
            <strong>Grok</strong>
            {card.truncated ? <span>Truncated</span> : null}
          </header>
          <MarkdownView text={card.text} onOpenUrl={onOpenUrl} />
        </article>
      );
    case "thought":
      return (
        <details className="stream-card stream-card--thought">
          <summary>
            Reasoning
            {card.truncated ? <span>Truncated</span> : null}
          </summary>
          <MarkdownView text={card.text} onOpenUrl={onOpenUrl} />
        </details>
      );
    case "tool":
      return (
        <article className={`stream-card stream-card--tool stream-card--${card.status}`}>
          <header>
            <strong>{card.title}</strong>
            <span>
              {card.kindLabel} · {card.statusLabel}
            </span>
          </header>
          {card.detail !== null ? (
            card.untrusted ? (
              <pre className="stream-card__terminal">
                {card.detail}
                {card.detailTruncated ? "\n[truncated]" : ""}
              </pre>
            ) : (
              <p className="stream-card__plain">{card.detail}</p>
            )
          ) : null}
        </article>
      );
    case "error":
      return (
        <article className="stream-card stream-card--error" role="alert">
          <header>
            <strong>{card.title}</strong>
          </header>
          <p className="stream-card__plain">{card.detail}</p>
        </article>
      );
  }
}
