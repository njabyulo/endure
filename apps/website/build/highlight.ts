import type { Plugin } from "vite";
import { createHighlighterCore } from "shiki/core";
import { createJavaScriptRegexEngine } from "shiki/engine/javascript";
import javascript from "shiki/langs/javascript.mjs";
import yaml from "shiki/langs/yaml.mjs";
import shellscript from "shiki/langs/shellscript.mjs";
import { featureExamples } from "../src/feature-data.ts";
import { INSTALL, APPROVAL } from "../src/installation-code.ts";

// Highlight our static examples at build time. The browser receives only tokens,
// with no highlighter, grammar downloads, WASM, or asynchronous color changes.
export async function highlightedExamples(): Promise<Plugin> {
  const highlighter = await createHighlighterCore({
    engine: createJavaScriptRegexEngine(),
    langs: [javascript, yaml, shellscript],
    themes: [{
      name: "ondure",
      type: "light",
      colors: { "editor.foreground": "#283d2e", "editor.background": "#f1f0e4" },
      tokenColors: [
        { scope: ["comment", "punctuation.definition.comment"], settings: { foreground: "#626953", fontStyle: "italic" } },
        { scope: ["keyword", "storage"], settings: { foreground: "#59632e" } },
        { scope: ["string"], settings: { foreground: "#805128" } },
        { scope: ["constant.numeric", "constant.language"], settings: { foreground: "#785264" } },
        { scope: ["entity.name.function", "support.function"], settings: { foreground: "#365c64" } },
        { scope: ["entity.name.tag", "entity.name.type", "support.type.property-name"], settings: { foreground: "#456039" } },
      ],
    }],
  });
  try {
    const snippets = [
      ...featureExamples.map(({ code, syntax }) => ({ code, lang: syntax ?? "javascript" })),
      { code: INSTALL, lang: "shellscript" },
      { code: APPROVAL, lang: "javascript" },
    ];
    const tokens = Object.fromEntries(snippets.map(({ code, lang }) => [
      code,
      highlighter.codeToTokens(code, { lang, theme: "ondure" }).tokens.map(line =>
        line.map(({ content, color, fontStyle }) => ({ content, color, fontStyle }))),
    ]));
    const id = "virtual:highlighted-examples";
    return {
      name: "ondure-highlighted-examples",
      resolveId(source) { if (source === id) return `\0${id}`; },
      load(source) { if (source === `\0${id}`) return `export default ${JSON.stringify(tokens)}`; },
    };
  } finally {
    highlighter.dispose();
  }
}
