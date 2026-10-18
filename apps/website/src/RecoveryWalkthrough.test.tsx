import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { RecoveryWalkthrough } from "./RecoveryWalkthrough";

describe("recovery walkthrough", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());
  it("plays interruption and replay before continuing", () => {
    render(<RecoveryWalkthrough />);
    fireEvent.click(screen.getByRole("button", { name: "Simulate interruption" }));
    for (const [index, name] of ["Record", "Interrupt", "Recover", "Continue"].entries()) {
      act(() => vi.advanceTimersByTime(1600));
      expect(screen.getByRole("status")).toHaveTextContent(`Step ${index + 2}: ${name}`);
      if (name === "Interrupt") expect(screen.getByText("Offline")).toBeInTheDocument();
    }
    act(() => vi.advanceTimersByTime(1600));
    expect(screen.getByRole("button", { name: "Simulate interruption" })).toBeEnabled();
  });
  it("manual selection and reset cancel automatic progression", () => {
    render(<RecoveryWalkthrough />);
    fireEvent.click(screen.getByRole("button", { name: "Simulate interruption" }));
    fireEvent.click(screen.getByRole("button", { name: "04 Recover" }));
    act(() => vi.advanceTimersByTime(10000));
    expect(screen.getByRole("status")).toHaveTextContent("Step 4: Recover");
    fireEvent.click(screen.getByRole("button", { name: "Reset recovery walkthrough" }));
    expect(screen.getByRole("status")).toHaveTextContent("Step 1: Run");
    expect(vi.getTimerCount()).toBe(0);
  });
  it("cancels playback on unmount", () => {
    const { unmount } = render(<RecoveryWalkthrough />);
    fireEvent.click(screen.getByRole("button", { name: "Simulate interruption" }));
    unmount();
    expect(vi.getTimerCount()).toBe(0);
  });
});
