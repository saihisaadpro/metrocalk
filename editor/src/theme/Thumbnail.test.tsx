//! The Thumbnail primitive (M14.2 / ADR-058) — verified headless: it renders the real image when the store
//! has a ready render, else the styled type-icon fallback. Keys off the STRUCTURED `data-thumb-status`
//! (`ready`/`fallback`), never a styled string or colour.

import { afterEach, expect, test } from "vitest";
import { act, render, screen } from "@testing-library/react";
import { Thumbnail } from "./Thumbnail";
import { thumbnailStore } from "../store/thumbnails";

afterEach(() => thumbnailStore.getState().reset());

test("no render yet → the type-icon fallback (status 'fallback')", () => {
  render(<Thumbnail id="x" kind="mesh" />);
  expect(screen.getByTestId("thumb").getAttribute("data-thumb-status")).toBe("fallback");
  expect(screen.getByTestId("type-icon")).toBeTruthy();
});

test("a ready render → the real image (status 'ready', no icon)", () => {
  render(<Thumbnail id="x" kind="mesh" />);
  act(() => thumbnailStore.getState().receive("x", "data:image/png;base64,AAAA"));
  const t = screen.getByTestId("thumb");
  expect(t.getAttribute("data-thumb-status")).toBe("ready");
  expect(t.querySelector("img")).toBeTruthy();
  expect(screen.queryByTestId("type-icon")).toBeNull();
});

test("a null render → fallback (over budget / offline / dev/browser)", () => {
  render(<Thumbnail id="x" kind="requirer" />);
  act(() => thumbnailStore.getState().receive("x", null));
  expect(screen.getByTestId("thumb").getAttribute("data-thumb-status")).toBe("fallback");
});
