import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { FeatureExamples } from "./FeatureExamples";
import { featureExamples } from "./feature-data";

describe("feature examples", () => {
  it("copies the selected feature and exposes its runnable source link", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
    render(<FeatureExamples examples={featureExamples} />);
    fireEvent.click(screen.getByRole("button", { name: "Copy Memory primitives example" }));
    await waitFor(() => expect(screen.getByText("Example copied.")).toBeInTheDocument());
    expect(writeText).toHaveBeenCalledWith(featureExamples[2].code);
    expect(screen.getByRole("heading", { name: "Standard interfaces" })).toBeInTheDocument();
    expect(screen.getAllByRole("link", { name: "Run this example" })).toHaveLength(7);
  });
  it("copies a dollar budget with nonzero model prices and tool estimates", async () => {
    const budget = featureExamples.find(({ title }) => title === "Budgets and scheduling")!;
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
    render(<FeatureExamples examples={[budget]} />);
    fireEvent.click(screen.getByRole("button", { name: "Copy Budgets and scheduling example" }));
    await waitFor(() => expect(writeText).toHaveBeenCalledWith(budget.code));
    // These ResourceLimits fields default to zero. A dollar cap without all
    // three prices would silently treat model or tool dispatch as free.
    for (const key of [
      "dollar_limit_micros",
      "input_price_micros_per_million_tokens",
      "output_price_micros_per_million_tokens",
      "tool_estimate_micros",
    ]) {
      const match = budget.code.match(new RegExp(`^  ${key}: ([0-9]+)(?: #.*)?$`, "m"));
      expect(match, `missing supported resource key ${key}`).not.toBeNull();
      expect(Number(match?.[1]), `${key} must account for nonzero spend`).toBeGreaterThan(0);
    }
    expect(screen.getByText(/sample rates below are illustrative/)).toBeInTheDocument();
  });
  it("keeps code available when clipboard access fails", async () => {
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText: vi.fn().mockRejectedValue(new Error("denied")) } });
    render(<FeatureExamples examples={[featureExamples[0]]} />);
    fireEvent.click(screen.getByRole("button", { name: "Copy Durable execution example" }));
    await waitFor(() => expect(screen.getByText("Select the code to copy manually.")).toBeInTheDocument());
    expect(screen.getByLabelText("Durable execution code")).toHaveTextContent("ctx.model.call");
  });
});
