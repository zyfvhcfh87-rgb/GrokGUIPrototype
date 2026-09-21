import { memo, useMemo, useRef, type KeyboardEvent, type MouseEvent, type ReactNode } from "react";

import type { MarkdownBlock, MarkdownInline } from "../application/markdown.ts";
import { parseMarkdown } from "../application/markdown.ts";

function MarkdownViewInner({
  text,
  onOpenUrl,
}: {
  text: string;
  onOpenUrl: (href: string) => void;
}) {
  const onOpenUrlRef = useRef(onOpenUrl);
  onOpenUrlRef.current = onOpenUrl;
  const blocks = useMemo(() => parseMarkdown(text), [text]);
  const openUrl = (href: string) => {
    onOpenUrlRef.current(href);
  };
  return (
    <div className="markdown">
      {blocks.map((block, index) => (
        <MarkdownBlockView
          key={`${block.type}-${index}`}
          block={block}
          onOpenUrl={openUrl}
        />
      ))}
    </div>
  );
}

export const MarkdownView = memo(MarkdownViewInner, (previous, next) => previous.text === next.text);

function MarkdownBlockView({
  block,
  onOpenUrl,
}: {
  block: MarkdownBlock;
  onOpenUrl: (href: string) => void;
}) {
  switch (block.type) {
    case "paragraph":
      return <p>{renderInlines(block.children, onOpenUrl)}</p>;
    case "heading": {
      const Tag = `h${block.level}` as const;
      return <Tag>{renderInlines(block.children, onOpenUrl)}</Tag>;
    }
    case "list": {
      const Tag = block.ordered ? "ol" : "ul";
      return (
        <Tag>
          {block.items.map((item, index) => (
            <li key={index}>{renderInlines(item, onOpenUrl)}</li>
          ))}
        </Tag>
      );
    }
    case "code_block":
      return (
        <pre>
          <code>{block.value}</code>
        </pre>
      );
    case "blockquote":
      return <blockquote>{renderInlines(block.children, onOpenUrl)}</blockquote>;
  }
}

function renderInlines(
  nodes: MarkdownInline[],
  onOpenUrl: (href: string) => void,
): ReactNode {
  return nodes.map((node, index) => {
    switch (node.type) {
      case "text":
        return <span key={index}>{node.value}</span>;
      case "code":
        return <code key={index}>{node.value}</code>;
      case "strong":
        return <strong key={index}>{renderInlines(node.children, onOpenUrl)}</strong>;
      case "emphasis":
        return <em key={index}>{renderInlines(node.children, onOpenUrl)}</em>;
      case "link":
        return (
          <a
            key={index}
            href={node.href}
            onClick={(event) => handleLink(event, node.href, onOpenUrl)}
            onKeyDown={(event) => handleLinkKey(event, node.href, onOpenUrl)}
          >
            {renderInlines(node.children, onOpenUrl)}
          </a>
        );
    }
  });
}

function handleLink(
  event: MouseEvent<HTMLAnchorElement>,
  href: string,
  onOpenUrl: (href: string) => void,
) {
  event.preventDefault();
  onOpenUrl(href);
}

function handleLinkKey(
  event: KeyboardEvent<HTMLAnchorElement>,
  href: string,
  onOpenUrl: (href: string) => void,
) {
  if (event.key === "Enter" || event.key === " ") {
    event.preventDefault();
    onOpenUrl(href);
  }
}
