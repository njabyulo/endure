import { highlightedExamples } from "./build/highlight.ts";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

// https://vite.dev/config/
export default defineConfig(async () => ({
  plugins: [react(), await highlightedExamples()],
}));
