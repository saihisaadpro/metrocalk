//! What an icon set has to be true of, that `tsc` cannot see.
//!
//! `Icon`'s `name` is a `string` on purpose — the shape/role/VFX/cinema/condition catalogs feed it at
//! runtime from Rust — so the compiler checks nothing about the vocabulary. `check-icon-vocab.mjs`
//! compares the two languages at rest; these are the claims that need the component to actually run:
//! that every entry DRAWS something, that an alias reports the drawing it resolved to rather than the
//! name that was asked for, and that an unknown name fails LOUDLY instead of quietly rendering a hole.

import { expect, test } from "vitest";
import { render, screen } from "@testing-library/react";
import { Icon, iconTokens, resolveIcon } from "./icons";
import { TypeIcon } from "./primitives";
import { control } from "./tokens";

/** Anything that puts ink on the 24-unit grid. */
const MARKS = "path, rect, circle, ellipse, line, polygon, polyline";

test("every name in the set — drawing or declared alias — actually draws something", () => {
  const tokens = iconTokens();
  expect(tokens.length).toBeGreaterThan(100); // the whole editor's vocabulary, not a handful

  const blank: string[] = [];
  for (const token of tokens) {
    const { container, unmount } = render(<Icon name={token} />);
    const svg = container.querySelector("svg");
    if (!svg || svg.querySelectorAll(MARKS).length === 0) blank.push(token);
    unmount();
  }
  // Named, not counted: a bare count tells you something broke and not which mark is an empty button —
  // which is the exact failure (`animation-timeline-tracks.png`) this whole set was built to end.
  expect(blank).toEqual([]);
});

test("an alias reports the DRAWING it resolved to, so a test can key on one name for one picture", () => {
  // `collectible` and `gem` are the same picture by declaration. If `data-icon` echoed the token asked
  // for, every catalog kind would need its own selector and the aliases would buy nothing.
  render(<Icon name="collectible" />);
  const svg = document.querySelector("[data-icon]");
  expect(svg?.getAttribute("data-icon")).toBe("gem");
  expect(svg?.hasAttribute("data-icon-missing")).toBe(false);
  expect(resolveIcon("collectible")).toBe("gem");
});

test("an unknown name draws NOTHING and says so — it never guesses a substitute", () => {
  const { container } = render(<Icon name="not-an-icon" />);
  const svg = container.querySelector("svg")!;
  expect(svg.getAttribute("data-icon-missing")).toBe("true");
  expect(svg.getAttribute("data-icon")).toBe("not-an-icon"); // the name that failed, for the report
  expect(svg.querySelectorAll(MARKS).length).toBe(0);
  // Still the right SIZE, so a missing mark leaves a hole in the layout rather than reflowing the row
  // around it — the defect must look like a defect, not like a slightly different design.
  expect(svg.getAttribute("width")).toBe(String(control.icon.md));
  expect(resolveIcon("not-an-icon")).toBeNull();
});

test("a fallback is used only when the name misses, and then the icon is no longer 'missing'", () => {
  const { container } = render(<Icon name="not-an-icon" fallback="shape" />);
  const svg = container.querySelector("svg")!;
  expect(svg.getAttribute("data-icon")).toBe("shape");
  expect(svg.hasAttribute("data-icon-missing")).toBe(false);
  expect(svg.querySelectorAll(MARKS).length).toBeGreaterThan(0);
});

test("size comes from the control.icon tokens, never from a magic number at the call site", () => {
  const { container } = render(
    <>
      <Icon name="play" size="sm" />
      <Icon name="play" size="xl" />
      <Icon name="play" size={13} />
    </>,
  );
  const [sm, xl, exact] = [...container.querySelectorAll("svg")];
  expect(sm.getAttribute("width")).toBe(String(control.icon.sm));
  expect(xl.getAttribute("width")).toBe(String(control.icon.xl));
  expect(exact.getAttribute("width")).toBe("13"); // an explicit px stays explicit
  // One grid for the whole family — the thing a per-call-site character could never promise.
  for (const svg of [sm, xl, exact]) expect(svg.getAttribute("viewBox")).toBe("0 0 24 24");
});

test("an icon is hidden from the reader unless it is asked to speak", () => {
  const { container, unmount } = render(<Icon name="play" />);
  expect(container.querySelector("svg")!.getAttribute("aria-hidden")).toBe("true");
  unmount();

  // A standalone mark with no labelled control around it opts into a name.
  render(<Icon name="play" title="Playing" />);
  const labelled = screen.getByRole("img");
  expect(labelled.getAttribute("aria-hidden")).toBeNull();
  expect(labelled.textContent).toContain("Playing");
});

test("TypeIcon draws the mark that matches its kind, and falls back rather than blanking", () => {
  const { container } = render(
    <>
      <TypeIcon kind="light" />
      <TypeIcon kind="a-kind-the-core-invented-today" />
    </>,
  );
  const [known, unknown] = [...container.querySelectorAll("[data-testid='type-icon']")];
  expect(known.querySelector("[data-icon]")?.getAttribute("data-icon")).toBe("light");
  // The fallback is what keeps a NEW core `kind` from photographing as an empty frame; `data-kind` is
  // still the structured signal, so the panel can be told apart from a genuinely-missing icon.
  expect(unknown.getAttribute("data-kind")).toBe("a-kind-the-core-invented-today");
  expect(unknown.querySelector("[data-icon]")?.getAttribute("data-icon")).toBe("shape");
});
