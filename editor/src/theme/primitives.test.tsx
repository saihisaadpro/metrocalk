//! Primitive state-matrix (M14.1 / ADR-057) — the shared `Button` is the control every toolbar/menu/bar
//! uses, so its variant/disabled/active/compact states are asserted here against the STRUCTURED signal (the
//! `mtk-btn*` class + the forwarded `disabled`/`data-testid`/`onClick`), never a styled colour string (the
//! marketplace-drift lesson, `<test_and_ci_discipline>` rule 3). The visual states themselves live in CSS.

import { expect, test, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { Button, Badge } from "./primitives";

test("Button: each variant maps to its stable class (the structured signal a restyle can't silently drift)", () => {
  const variants = ["primary", "secondary", "ghost", "danger", "toggle"] as const;
  for (const v of variants) {
    render(
      <Button variant={v} data-testid={`btn-${v}`}>
        {v}
      </Button>,
    );
    const el = screen.getByTestId(`btn-${v}`);
    expect(el.className).toContain("mtk-btn");
    expect(el.className).toContain(`mtk-btn--${v}`);
  }
});

test("Button: toggle reflects live `active` as `.is-active` (so live tool/snap/space state is unmistakable)", () => {
  const { rerender } = render(
    <Button variant="toggle" active={false} data-testid="tg">
      Snap
    </Button>,
  );
  expect(screen.getByTestId("tg").className).not.toContain("is-active");
  rerender(
    <Button variant="toggle" active data-testid="tg">
      Snap
    </Button>,
  );
  expect(screen.getByTestId("tg").className).toContain("is-active");
});

test("Button: compact + icon modifiers add their classes", () => {
  render(
    <Button compact icon data-testid="c">
      ▣
    </Button>,
  );
  const el = screen.getByTestId("c");
  expect(el.className).toContain("mtk-btn--compact");
  expect(el.className).toContain("mtk-btn--icon");
});

test("Button: disabled forwards to the element AND blocks the click (no enabled-inert CTA — C5)", () => {
  const onClick = vi.fn();
  render(
    <Button disabled data-testid="d" onClick={onClick}>
      Go
    </Button>,
  );
  const el = screen.getByTestId("d") as HTMLButtonElement;
  expect(el.disabled).toBe(true);
  fireEvent.click(el);
  expect(onClick).not.toHaveBeenCalled();
});

test("Button: forwards id + click for the prompt-40 selectors", () => {
  const onClick = vi.fn();
  render(
    <Button id="myBtn" data-testid="m" onClick={onClick}>
      Go
    </Button>,
  );
  expect(screen.getByTestId("m").id).toBe("myBtn");
  fireEvent.click(screen.getByTestId("m"));
  expect(onClick).toHaveBeenCalledTimes(1);
});

test("Badge: tone selects a structured style without throwing (smoke)", () => {
  render(
    <Badge tone="accent" style={{}}>
      <span data-testid="badge-child">x</span>
    </Badge>,
  );
  expect(screen.getByTestId("badge-child")).toBeTruthy();
});
