import { describe, expect, it } from "vitest";
import { parseInline, parseMarkdown } from "./markdown";

describe("parseInline", () => {
  it("parses bold, italic and inline code", () => {
    const spans = parseInline("use **Open Project** then `npm run` *fast*");
    expect(spans).toEqual([
      { type: "text", value: "use " },
      { type: "bold", value: "Open Project" },
      { type: "text", value: " then " },
      { type: "code", value: "npm run" },
      { type: "text", value: " " },
      { type: "italic", value: "fast" },
    ]);
  });

  it("returns plain text untouched", () => {
    expect(parseInline("salaam bhai")).toEqual([{ type: "text", value: "salaam bhai" }]);
  });
});

describe("parseMarkdown", () => {
  it("parses a fenced code block with language", () => {
    const blocks = parseMarkdown("```csharp\nvoid Start() {}\n```");
    expect(blocks).toEqual([{ type: "code", lang: "csharp", content: "void Start() {}" }]);
  });

  it("parses headings, bullets and numbered lists", () => {
    const blocks = parseMarkdown("## Plan\n- first item\n1. second step\nnormal text");
    const lineTypes = blocks
      .filter((b) => b.type === "text")
      .flatMap((b) => (b.type === "text" ? b.lines.map((l) => l.type) : []));
    expect(lineTypes).toEqual(["heading", "bullet", "numbered", "para"]);
  });

  it("keeps surrounding paragraphs around code", () => {
    const blocks = parseMarkdown("before\n```\ncode\n```\nafter");
    expect(blocks.map((b) => b.type)).toEqual(["text", "code", "text"]);
  });

  it("never throws on unclosed fences", () => {
    const blocks = parseMarkdown("```csharp\nvoid Start() {}");
    expect(blocks).toEqual([{ type: "code", lang: "csharp", content: "void Start() {}" }]);
  });

  it("handles empty input", () => {
    expect(parseMarkdown("")).toEqual([]);
  });
});
