/// <reference types="node" />
import { existsSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { featureExamples } from "./feature-data";

describe("runnable example references", () => {
  it("links every feature to a real repository example", () => {
    for (const example of featureExamples) {
      const file = resolve(process.cwd(), "../..", example.path);
      expect(existsSync(file), `${example.title}: ${example.path}`).toBe(true);
    }
  });
  it("provides valid JavaScript handler excerpts", () => {
    for (const example of featureExamples.filter(example => !example.language)) {
      expect(() => new Function(`return async function(ctx, input) {\n${example.code}\n}`)).not.toThrow();
    }
  });
});
