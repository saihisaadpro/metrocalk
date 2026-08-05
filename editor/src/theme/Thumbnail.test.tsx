//! The Thumbnail primitive (M14.2 / ADR-058) — verified headless: it renders the real image when the store
//! has a ready render, else the styled type-icon fallback. Keys off the STRUCTURED `data-thumb-status`
//! (`ready`/`fallback`), never a styled string or colour.

import { afterEach, expect, test } from "vitest";
import { act, fireEvent, render, screen } from "@testing-library/react";
import { Thumbnail } from "./Thumbnail";
import { thumbnailStore } from "../store/thumbnails";

afterEach(() => act(() => thumbnailStore.getState().reset()));

test("no render yet → the type-icon fallback (status 'fallback')", () => {
  render(<Thumbnail id="x" kind="mesh" />);
  expect(screen.getByTestId("thumb").getAttribute("data-thumb-status")).toBe("fallback");
  expect(screen.getByTestId("type-icon")).toBeTruthy();
});

test("a ready render → the real image (status 'ready', no icon)", async () => {
  render(<Thumbnail id="x" kind="mesh" />);
  await act(async () => thumbnailStore.getState().receive("x", "data:image/png;base64,AAAA"));
  const t = screen.getByTestId("thumb");
  expect(t.getAttribute("data-thumb-status")).toBe("ready");
  const image = t.querySelector("img")!;
  expect(image).toBeTruthy();
  expect(image.getAttribute("width")).toBe("40");
  expect(image.getAttribute("height")).toBe("40");
  expect(image.getAttribute("decoding")).toBe("async");
  expect(image.getAttribute("alt")).toBe("");
  expect(screen.queryByTestId("type-icon")).toBeNull();
});

test("a PNG decode failure returns to the stable icon fallback", async () => {
  render(<Thumbnail id="x" kind="mesh" size={56} />);
  await act(async () => thumbnailStore.getState().receive("x", "data:image/png;base64,broken"));
  await act(async () => fireEvent.error(screen.getByTestId("thumb").querySelector("img")!));
  expect(screen.getByTestId("thumb").getAttribute("data-thumb-status")).toBe("fallback");
  expect(screen.getByTestId("type-icon")).toBeTruthy();
});

test("a null render → fallback (over budget / offline / dev/browser)", async () => {
  render(<Thumbnail id="x" kind="requirer" />);
  await act(async () => thumbnailStore.getState().receive("x", null));
  expect(screen.getByTestId("thumb").getAttribute("data-thumb-status")).toBe("fallback");
});
