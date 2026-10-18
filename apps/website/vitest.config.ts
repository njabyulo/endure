import { highlightedExamples } from "./build/highlight.ts";
import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";
export default defineConfig(async () => ({
  plugins: [react(), await highlightedExamples()],
  test: { environment: "jsdom", setupFiles: ["./src/test-setup.ts"] },
}));
