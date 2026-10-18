import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { HighlightedCode } from "./HighlightedCode";
import { featureExamples } from "./feature-data";
import { INSTALL, APPROVAL } from "./installation-code";
import App from "./App";

describe("Shiki code rendering", () => {
  it.each([
    ["JavaScript", featureExamples[0].code],
    ["YAML", featureExamples.find(example => example.syntax === "yaml")!.code],
    ["shell", featureExamples.find(example => example.syntax === "shellscript")!.code],
    ["installation commands", INSTALL],
    ["approval", APPROVAL],
  ])("highlights %s while preserving its exact source", (_language, code) => {
    const { container } = render(<pre><HighlightedCode code={code} /></pre>);
    const rendered = container.querySelector("code")!;
    expect(rendered.textContent).toBe(code);
    const spans = [...rendered.querySelectorAll("span")];
    expect(spans.length).toBeGreaterThan(1);
    expect(new Set(spans.map(span => span.style.color)).size).toBeGreaterThan(1);
  });

  it("renders unregistered code as escaped text", () => {
    const code = '<script>alert("example")</script>\n<img src=x onerror=alert(1)>\n';
    const { container } = render(<pre><HighlightedCode code={code} /></pre>);
    expect(container.querySelector("code")!.textContent).toBe(code);
    expect(container.querySelector("script, img")).toBeNull();
  });

  it("keeps highlighted installation and feature blocks keyboard accessible", () => {
    render(<App />);
    for (const name of ["Build and run commands", "Durable approval example", ...featureExamples.map(example => `${example.title} code`)]) {
      const block = screen.getByLabelText(name);
      expect(block.tagName).toBe("PRE");
      expect(block).toHaveAttribute("tabindex", "0");
      expect(block.querySelector("code.shiki-code span")).not.toBeNull();
    }
  });
});
