import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { expect, test, vi } from "vitest";
import { EditorHeader } from "./EditorHeader";
import { fakeClient } from "../transport/test-client";

test("exposes honest undo and redo actions in the primary editor toolbar", async () => {
  const undo = vi.fn(() => Promise.resolve(true));
  const redo = vi.fn(() => Promise.resolve(true));
  render(
    <EditorHeader
      client={fakeClient({ undo, redo })}
      onOpenCreate={vi.fn()}
      onOpenLogic={vi.fn()}
      onOpenReview={vi.fn()}
      onOpenCommands={vi.fn()}
    />,
  );

  fireEvent.click(screen.getByTestId("header-undo"));
  fireEvent.click(screen.getByTestId("header-redo"));
  await waitFor(() => {
    expect(undo).toHaveBeenCalledTimes(1);
    expect(redo).toHaveBeenCalledTimes(1);
  });
  expect(screen.getByTestId("header-redo").getAttribute("title")).toMatch(/shift\+z/i);
  expect(screen.getByRole("group", { name: "Play controls" })).toBeTruthy();
  expect(screen.getByRole("group", { name: "Optional AI credits" })).toBeTruthy();
});

test("keeps both dock drawers and command search reachable in the compact header", () => {
  const openLeft = vi.fn();
  const openRight = vi.fn();
  const openCommands = vi.fn();
  const openLogic = vi.fn();
  render(
    <EditorHeader
      client={fakeClient()}
      compact
      onOpenCreate={vi.fn()}
      onOpenLogic={openLogic}
      onOpenReview={vi.fn()}
      onOpenCommands={openCommands}
      onOpenLeftDock={openLeft}
      onOpenRightDock={openRight}
    />,
  );

  const scene = screen.getByTestId("header-scene");
  const inspector = screen.getByTestId("header-inspector");
  const commands = screen.getByTestId("command-palette-trigger");
  const workspaces = screen.getByTestId("header-workspaces");
  expect(screen.getByTestId("editor-header").className).toContain("is-compact");
  expect(scene.closest(".mtk-editor-header__start")).not.toBeNull();
  expect(inspector.closest(".mtk-editor-header__start")).not.toBeNull();
  expect(commands.closest(".mtk-editor-header__start")).not.toBeNull();

  fireEvent.click(scene);
  fireEvent.click(inspector);
  fireEvent.click(commands);
  fireEvent.click(workspaces);
  expect(screen.getByRole("menu", { name: "Open engine workspace" })).toBeTruthy();
  fireEvent.click(screen.getByTestId("header-logic"));
  expect(openLeft).toHaveBeenCalledTimes(1);
  expect(openRight).toHaveBeenCalledTimes(1);
  expect(openCommands).toHaveBeenCalledTimes(1);
  expect(openLogic).toHaveBeenCalledTimes(1);
});
