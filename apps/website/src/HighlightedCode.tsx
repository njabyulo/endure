import { Fragment } from "react";
import examples from "virtual:highlighted-examples";

/** Render Shiki tokens as text nodes, preserving source and escaping markup. */
export function HighlightedCode({ code }: { code: string }) {
  const lines = examples[code];
  return <code className={lines ? "shiki-code" : undefined}>{lines
    ? lines.map((line, index) => <Fragment key={index}>
      {index > 0 ? "\n" : null}
      {line.map((token, tokenIndex) => <span key={tokenIndex} style={{
        color: token.color,
        fontStyle: token.fontStyle && token.fontStyle & 1 ? "italic" : undefined,
        fontWeight: token.fontStyle && token.fontStyle & 2 ? "bold" : undefined,
        textDecoration: token.fontStyle && token.fontStyle & 4 ? "underline" : undefined,
      }}>{token.content}</span>)}
    </Fragment>)
    : code}</code>;
}
