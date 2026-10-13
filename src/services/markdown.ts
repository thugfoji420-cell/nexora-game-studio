/**
 * Tiny zero-dependency Markdown parser for chat replies.
 *
 * Supports the subset an LLM assistant actually uses: fenced code blocks,
 * headings, bullet / numbered lists, **bold**, *italic* and `inline code`.
 * Returns plain data structures (React renders them safely — no HTML injection).
 */

export type MdSpan =
  | { type: "text"; value: string }
  | { type: "bold"; value: string }
  | { type: "italic"; value: string }
  | { type: "code"; value: string };

export type MdLine =
  | { type: "heading"; level: number; spans: MdSpan[] }
  | { type: "bullet"; spans: MdSpan[] }
  | { type: "numbered"; n: number; spans: MdSpan[] }
  | { type: "para"; spans: MdSpan[] };

export type MdBlock =
  | { type: "code"; lang: string; content: string }
  | { type: "text"; lines: MdLine[] };

const INLINE_RE = /(\*\*[^*]+\*\*|`[^`]+`|\*[^*\s][^*]*\*)/g;

/** Parse one line's inline formatting: **bold**, `code`, *italic*. */
export function parseInline(text: string): MdSpan[] {
  const spans: MdSpan[] = [];
  let last = 0;
  for (const match of text.matchAll(INLINE_RE)) {
    const idx = match.index ?? 0;
    if (idx > last) spans.push({ type: "text", value: text.slice(last, idx) });
    const token = match[0];
    if (token.startsWith("**")) spans.push({ type: "bold", value: token.slice(2, -2) });
    else if (token.startsWith("`")) spans.push({ type: "code", value: token.slice(1, -1) });
    else spans.push({ type: "italic", value: token.slice(1, -1) });
    last = idx + token.length;
  }
  if (last < text.length) spans.push({ type: "text", value: text.slice(last) });
  return spans.length ? spans : [{ type: "text", value: text }];
}

/** Parse full reply text into renderable blocks. Never throws. */
export function parseMarkdown(text: string): MdBlock[] {
  const blocks: MdBlock[] = [];
  const lines = text.replace(/\r\n/g, "\n").split("\n");
  let para: string[] = [];
  const flushPara = () => {
    if (para.length) {
      blocks.push({
        type: "text",
        lines: para.map((line) => ({ type: "para", spans: parseInline(line) })),
      });
      para = [];
    }
  };

  let i = 0;
  while (i < lines.length) {
    const line = lines[i];

    const fence = line.match(/^\s*```(\w*)\s*$/);
    if (fence) {
      flushPara();
      const lang = fence[1] ?? "";
      const content: string[] = [];
      i++;
      while (i < lines.length && !/^\s*```\s*$/.test(lines[i])) {
        content.push(lines[i]);
        i++;
      }
      i++; // skip the closing fence (or EOF)
      blocks.push({ type: "code", lang, content: content.join("\n") });
      continue;
    }

    const heading = line.match(/^(#{1,4})\s+(.*)$/);
    if (heading) {
      flushPara();
      blocks.push({
        type: "text",
        lines: [{ type: "heading", level: heading[1].length, spans: parseInline(heading[2]) }],
      });
      i++;
      continue;
    }

    const bullet = line.match(/^\s*[-*•]\s+(.*)$/);
    if (bullet) {
      flushPara();
      blocks.push({ type: "text", lines: [{ type: "bullet", spans: parseInline(bullet[1]) }] });
      i++;
      continue;
    }

    const numbered = line.match(/^\s*(\d+)[.)]\s+(.*)$/);
    if (numbered) {
      flushPara();
      blocks.push({
        type: "text",
        lines: [{ type: "numbered", n: Number(numbered[1]), spans: parseInline(numbered[2]) }],
      });
      i++;
      continue;
    }

    if (line.trim() === "") {
      flushPara();
      i++;
      continue;
    }

    para.push(line);
    i++;
  }
  flushPara();
  return blocks;
}
