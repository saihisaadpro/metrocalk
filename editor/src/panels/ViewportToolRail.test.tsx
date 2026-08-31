import { fireEvent, render, screen } from "@testing-library/react";
import { expect, test, vi } from "vitest";
import { ViewportToolRail } from "./ViewportToolRail";

test("exposes a controlled vertical toolbar with live selection and real shortcuts", () => {
  const onToolChange = vi.fn();
  render(<ViewportToolRail activeTool="move" onToolChange={onToolChange} data-testid="rail" />);

  expect(screen.getByRole("navigation", { name: "Viewport tools" })).toBeTruthy();
  const toolbar = screen.getByRole("toolbar", { name: "Primary viewport tools" });
  expect(toolbar.getAttribute("aria-orientation")).toBe("vertical");
  expect(screen.getAllByRole("button").filter((button) => button.hasAttribute("data-tool"))).toHaveLength(6);
  expect(screen.getByRole("button", { name: "Move" }).getAttribute("aria-pressed")).toBe("true");
  expect(screen.getByRole("button", { name: "Move" }).id).toBe("vpMove");
  expect(screen.getByRole("button", { name: "Move" }).title).toContain("(W)");
  expect(screen.getByRole("button", { name: "Pipe" }).title).not.toMatch(/\([A-Z]\)/);
  // The ground sketch is a primary tool with a primary tool's shortcut, beside W/E/R.
  expect(screen.getByRole("button", { name: "Draw" }).id).toBe("vpDraw");
  expect(screen.getByRole("button", { name: "Draw" }).title).toContain("(D)");

  fireEvent.click(screen.getByRole("button", { name: "Rotate" }));
  expect(onToolChange).toHaveBeenCalledWith("rotate");
  // Controlled means the rail waits for its owner instead of presenting speculative state.
  expect(screen.getByRole("button", { name: "Move" }).getAttribute("aria-pressed")).toBe("true");
});

test("roving arrow, Home, and End focus skips disabled and hidden tools without activating them", () => {
  const onToolChange = vi.fn();
  render(
    <ViewportToolRail
      activeTool="select"
      onToolChange={onToolChange}
      availability={{ move: { disabled: true, reason: "Select an object first" }, scale: { hidden: true } }}
    />,
  );
  const select = screen.getByRole("button", { name: "Select" });
  const rotate = screen.getByRole("button", { name: "Rotate" });
  select.focus();

  fireEvent.keyDown(select, { key: "ArrowDown" });
  expect(document.activeElement).toBe(rotate);
  fireEvent.keyDown(rotate, { key: "End" });
  expect(document.activeElement).toBe(screen.getByRole("button", { name: "Pipe" }));
  fireEvent.keyDown(screen.getByRole("button", { name: "Pipe" }), { key: "Home" });
  expect(document.activeElement).toBe(select);
  fireEvent.keyDown(select, { key: "ArrowUp" });
  expect(document.activeElement).toBe(screen.getByRole("button", { name: "Pipe" }));
  expect(onToolChange).not.toHaveBeenCalled();
  expect(screen.queryByRole("button", { name: "Scale" })).toBeNull();
});

test("unavailable tools carry a visible tooltip reason and accessible description", () => {
  render(
    <ViewportToolRail
      activeTool="select"
      onToolChange={() => {}}
      availability={{ pipe: { disabled: true, reason: "Pipe tools are unavailable while playing" } }}
    />,
  );
  const pipe = screen.getByRole("button", { name: "Pipe" });
  expect(pipe.hasAttribute("disabled")).toBe(true);
  expect(pipe.title).toContain("Pipe tools are unavailable while playing");
  const describedBy = pipe.getAttribute("aria-describedby");
  expect(describedBy).toBeTruthy();
  expect(document.getElementById(describedBy!)?.textContent).toBe("Pipe tools are unavailable while playing");
});

test("local minimize preserves tool access and can be expanded again", () => {
  const onMinimizedChange = vi.fn();
  render(<ViewportToolRail activeTool="select" onToolChange={() => {}} onMinimizedChange={onMinimizedChange} data-testid="rail" />);
  const rail = screen.getByTestId("rail");
  expect(rail.getAttribute("data-minimized")).toBe("false");
  expect(screen.getByText("Select")).toBeTruthy();

  fireEvent.click(screen.getByRole("button", { name: "Collapse viewport tool names" }));
  expect(rail.getAttribute("data-minimized")).toBe("true");
  expect(screen.queryByText("Select")).toBeNull();
  expect(screen.getByRole("button", { name: "Select" })).toBeTruthy();
  expect(onMinimizedChange).toHaveBeenCalledWith(true);

  fireEvent.click(screen.getByRole("button", { name: "Expand viewport tool names" }));
  expect(rail.getAttribute("data-minimized")).toBe("false");
  expect(onMinimizedChange).toHaveBeenLastCalledWith(false);
});

test("a controlled minimize request is reported without changing presentation", () => {
  const onMinimizedChange = vi.fn();
  render(
    <ViewportToolRail activeTool="select" onToolChange={() => {}} minimized={false} onMinimizedChange={onMinimizedChange} data-testid="rail" />,
  );
  fireEvent.click(screen.getByRole("button", { name: "Collapse viewport tool names" }));
  expect(onMinimizedChange).toHaveBeenCalledWith(true);
  expect(screen.getByTestId("rail").getAttribute("data-minimized")).toBe("false");
});

test("tool interactions do not fall through to viewport gestures", () => {
  const parentClick = vi.fn();
  const parentPointer = vi.fn();
  const onToolChange = vi.fn();
  render(
    <div onClick={parentClick} onPointerDown={parentPointer}>
      <ViewportToolRail activeTool="select" onToolChange={onToolChange} />
    </div>,
  );
  const pipe = screen.getByRole("button", { name: "Pipe" });
  fireEvent.pointerDown(pipe);
  fireEvent.click(pipe);
  expect(onToolChange).toHaveBeenCalledWith("pipe");
  expect(parentPointer).not.toHaveBeenCalled();
  expect(parentClick).not.toHaveBeenCalled();
});
