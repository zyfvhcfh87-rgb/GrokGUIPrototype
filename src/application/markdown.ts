import { validateExternalUrl } from "./urls.ts";

export type MarkdownInline =
  | { type: "text"; value: string }
  | { type: "code"; value: string }
  | { type: "strong"; children: MarkdownInline[] }
  | { type: "emphasis"; children: MarkdownInline[] }
  | { type: "link"; href: string; children: MarkdownInline[] };

export type MarkdownBlock =
  | { type: "paragraph"; children: MarkdownInline[] }
  | { type: "heading"; level: 1 | 2 | 3 | 4 | 5 | 6; children: MarkdownInline[] }
  | { type: "list"; ordered: boolean; items: MarkdownInline[][] }
  | { type: "code_block"; language: string | null; value: string }
  | { type: "blockquote"; children: MarkdownInline[] };

export function parseMarkdown(source: string): MarkdownBlock[] {
  const lines = source.replaceAll("\r\n", "\n").split("\n");
  const blocks: MarkdownBlock[] = [];
  let index = 0;

  while (index < lines.length) {
    const line = lines[index] ?? "";
    if (line.trim() === "") {
      index += 1;
      continue;
    }

    const fence = readFence(line);
    if (fence !== null) {
      const closed: string[] = [];
      index += 1;
      while (index < lines.length && !isClosingFence(lines[index] ?? "", fence.marker)) {
        closed.push(lines[index] ?? "");
        index += 1;
      }
      if (index < lines.length) {
        index += 1;
      }
      blocks.push({
        type: "code_block",
        language: fence.language,
        value: closed.join("\n"),
      });
      continue;
    }

    const heading = /^(#{1,6})\s+(.+)$/.exec(line);
    if (heading !== null) {
      const level = heading[1]?.length ?? 1;
      blocks.push({
        type: "heading",
        level: Math.min(level, 6) as 1 | 2 | 3 | 4 | 5 | 6,
        children: parseInlines(heading[2] ?? ""),
      });
      index += 1;
      continue;
    }

    if (/^([-*_])\1{2,}\s*$/.test(line.trim())) {
      index += 1;
      continue;
    }

    if (line.startsWith(">")) {
      const quoted: string[] = [];
      while (index < lines.length && (lines[index] ?? "").startsWith(">")) {
        quoted.push((lines[index] ?? "").replace(/^>\s?/, ""));
        index += 1;
      }
      blocks.push({
        type: "blockquote",
        children: parseInlines(quoted.join(" ")),
      });
      continue;
    }

    const unordered = /^[-*]\s+/.test(line);
    const ordered = /^\d+\.\s+/.test(line);
    if (unordered || ordered) {
      const items: MarkdownInline[][] = [];
      while (index < lines.length) {
        const item = lines[index] ?? "";
        const match = ordered ? /^\d+\.\s+(.+)$/.exec(item) : /^[-*]\s+(.+)$/.exec(item);
        if (match === null) {
          break;
        }
        items.push(parseInlines(match[1] ?? ""));
        index += 1;
      }
      blocks.push({ type: "list", ordered, items });
      continue;
    }

    const paragraph: string[] = [line];
    index += 1;
    while (index < lines.length) {
      const next = lines[index] ?? "";
      if (
        next.trim() === "" ||
        readFence(next) !== null ||
        /^(#{1,6})\s+/.test(next) ||
        next.startsWith(">") ||
        /^[-*]\s+/.test(next) ||
        /^\d+\.\s+/.test(next)
      ) {
        break;
      }
      paragraph.push(next);
      index += 1;
    }
    blocks.push({ type: "paragraph", children: parseInlines(paragraph.join(" ")) });
  }

  return blocks;
}

function readFence(line: string): { marker: string; language: string | null } | null {
  const match = /^(```|~~~)([^`~]*)$/.exec(line);
  if (match === null) {
    return null;
  }
  const language = (match[2] ?? "").trim();
  return {
    marker: match[1] ?? "```",
    language: language === "" ? null : language.replace(/[^A-Za-z0-9_+#-]/g, ""),
  };
}

function isClosingFence(line: string, marker: string): boolean {
  return line.trim() === marker;
}

export function parseInlines(input: string): MarkdownInline[] {
  const nodes: MarkdownInline[] = [];
  let cursor = 0;
  let textStart = 0;

  const flush = (end: number) => {
    if (end > textStart) {
      nodes.push({ type: "text", value: input.slice(textStart, end) });
    }
  };

  while (cursor < input.length) {
    if (input[cursor] === "`") {
      const end = input.indexOf("`", cursor + 1);
      if (end !== -1) {
        flush(cursor);
        nodes.push({ type: "code", value: input.slice(cursor + 1, end) });
        cursor = end + 1;
        textStart = cursor;
        continue;
      }
    }

    if (input.startsWith("**", cursor)) {
      const end = input.indexOf("**", cursor + 2);
      if (end !== -1) {
        flush(cursor);
        nodes.push({
          type: "strong",
          children: [{ type: "text", value: input.slice(cursor + 2, end) }],
        });
        cursor = end + 2;
        textStart = cursor;
        continue;
      }
    }

    if (input[cursor] === "*" && input[cursor + 1] !== "*") {
      const end = input.indexOf("*", cursor + 1);
      if (end !== -1) {
        flush(cursor);
        nodes.push({
          type: "emphasis",
          children: [{ type: "text", value: input.slice(cursor + 1, end) }],
        });
        cursor = end + 1;
        textStart = cursor;
        continue;
      }
    }

    if (input.startsWith("![", cursor)) {
      flush(cursor);
      cursor += 1;
      textStart = cursor;
      continue;
    }

    if (input[cursor] === "[") {
      const labelEnd = input.indexOf("](", cursor);
      const urlEnd = labelEnd === -1 ? -1 : findClosingParen(input, labelEnd + 2);
      if (labelEnd !== -1 && urlEnd !== -1) {
        const label = input.slice(cursor + 1, labelEnd);
        const href = input.slice(labelEnd + 2, urlEnd);
        const validated = validateExternalUrl(href);
        flush(cursor);
        if (validated.ok) {
          nodes.push({
            type: "link",
            href: validated.href,
            children: [{ type: "text", value: label }],
          });
        } else {
          nodes.push({ type: "text", value: label });
        }
        cursor = urlEnd + 1;
        textStart = cursor;
        continue;
      }
    }

    cursor += 1;
  }

  flush(input.length);
  return nodes.length > 0 ? nodes : [{ type: "text", value: "" }];
}

function findClosingParen(input: string, start: number): number {
  let depth = 1;
  for (let index = start; index < input.length; index += 1) {
    if (input[index] === "(") {
      depth += 1;
    } else if (input[index] === ")") {
      depth -= 1;
      if (depth === 0) {
        return index;
      }
    }
  }
  return -1;
}
