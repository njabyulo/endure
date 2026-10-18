import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import App from "./App";

describe("landing page controls", () => {
  it("links the declared license to its full repository terms", () => {
    render(<App />);
    expect(screen.getByRole("link", { name: "MIT license ↗" })).toHaveAttribute(
      "href", "https://github.com/njabyulo/endure/blob/feat/agent-runtime-website/LICENSE",
    );
  });
  it("opens and closes mobile navigation, including Escape and anchor selection", () => {
    render(<App />);
    const toggle = screen.getByRole("button", { name: "Open menu" });
    fireEvent.click(toggle);
    expect(toggle).toHaveAttribute("aria-expanded", "true");
    fireEvent.keyDown(toggle, { key: "Escape" });
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    expect(toggle).toHaveFocus();
    fireEvent.click(toggle);
    fireEvent.click(screen.getByRole("link", { name: "Recovery" }));
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    expect(screen.getByRole("link", { name: "Get started" })).toHaveAttribute(
      "href",
      "#get-started",
    );
  });
  it("copies the actual build commands", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText },
    });
    render(<App />);
    fireEvent.click(
      screen.getByRole("button", { name: "Copy build and run commands" }),
    );
    await waitFor(() =>
      expect(screen.getByText("Commands copied.")).toBeInTheDocument(),
    );
    expect(writeText).toHaveBeenCalledWith(
      expect.stringContaining("cargo build --bin ondure"),
    );
    expect(writeText).toHaveBeenCalledWith(
      expect.stringContaining("python3 examples/model-operations/run.py"),
    );
  });
  it("provides a manual-copy fallback when clipboard access fails", async () => {
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText: vi.fn().mockRejectedValue(new Error("denied")) },
    });
    render(<App />);
    fireEvent.click(
      screen.getByRole("button", { name: "Copy build and run commands" }),
    );
    await waitFor(() =>
      expect(screen.getByText(/Copy unavailable/)).toBeInTheDocument(),
    );
  });
});
