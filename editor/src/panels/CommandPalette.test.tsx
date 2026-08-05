import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { expect, test, vi } from "vitest";
import { CommandPalette, type EditorCommand } from "./CommandPalette";

function command(overrides: Partial<EditorCommand> & Pick<EditorCommand, "id" | "label">): EditorCommand {
  return {
    category: "General",
    execute: vi.fn(),
    ...overrides,
  };
}

const COMMANDS: EditorCommand[] = [
  command({ id: "new", label: "New scene", category: "Project", description: "Start with an empty scene", shortcut: ["Ctrl", "N"] }),
  command({ id: "save", label: "Save project", category: "Project", disabled: true, disabledReason: "Name the project before saving" }),
  command({ id: "frame", label: "Frame selection", category: "Viewport", description: "Center the camera on the selection", shortcut: "F" }),
  command({ id: "import", label: "Import asset", category: "Assets", keywords: ["mesh", "model", "glTF"] }),
];

test("renders a labelled modal, grouped commands, shortcuts, and autofocuses search", () => {
  const { rerender } = render(<CommandPalette open={false} onClose={() => {}} commands={COMMANDS} />);
  expect(screen.queryByRole("dialog")).toBeNull();

  rerender(<CommandPalette open onClose={() => {}} commands={COMMANDS} />);
  expect(screen.getByRole("dialog", { name: "Command palette" })).toBeTruthy();
  expect(screen.getByRole("heading", { name: "Command palette" })).toBeTruthy();
  const search = screen.getByRole("combobox", { name: "Search commands" });
  expect(document.activeElement).toBe(search);

  const results = screen.getByRole("listbox", { name: "Command results" });
  expect(within(results).getAllByRole("group")).toHaveLength(3);
  expect(within(results).getByText("Project")).toBeTruthy();
  expect(within(results).getByLabelText("Ctrl plus N")).toBeTruthy();
  expect(search.getAttribute("aria-activedescendant")).toBe(screen.getByRole("option", { name: /New scene/ }).id);
});

test("searches labels, descriptions, categories, and hidden keywords and presents an empty state", () => {
  render(<CommandPalette open onClose={() => {}} commands={COMMANDS} />);
  const search = screen.getByRole("combobox", { name: "Search commands" });

  fireEvent.change(search, { target: { value: "mesh" } });
  expect(screen.getAllByRole("option")).toHaveLength(1);
  expect(screen.getByRole("option", { name: /Import asset/ })).toBeTruthy();

  fireEvent.change(search, { target: { value: "camera" } });
  expect(screen.getByRole("option", { name: /Frame selection/ })).toBeTruthy();

  fireEvent.change(search, { target: { value: "does-not-exist" } });
  expect(screen.queryByRole("option")).toBeNull();
  expect(screen.getByRole("status").textContent).toContain("No matching commands");
  expect(search.getAttribute("aria-expanded")).toBe("false");
  expect(search.hasAttribute("aria-controls")).toBe(false);
  expect(search.hasAttribute("aria-activedescendant")).toBe(false);
});

test("arrow, Home, and End navigation wraps and skips unavailable commands", async () => {
  const runNew = vi.fn();
  const runFrame = vi.fn();
  const onClose = vi.fn();
  const commands = [
    command({ id: "new", label: "New", execute: runNew }),
    command({ id: "save", label: "Save", disabled: true, disabledReason: "No changes", execute: vi.fn() }),
    command({ id: "frame", label: "Frame", execute: runFrame }),
  ];
  render(<CommandPalette open onClose={onClose} commands={commands} />);
  const search = screen.getByRole("combobox", { name: "Search commands" });

  fireEvent.keyDown(search, { key: "ArrowDown" });
  expect(screen.getByRole("option", { name: "Frame" }).getAttribute("aria-selected")).toBe("true");
  fireEvent.keyDown(search, { key: "ArrowDown" });
  expect(screen.getByRole("option", { name: "New" }).getAttribute("aria-selected")).toBe("true");
  fireEvent.keyDown(search, { key: "End" });
  expect(screen.getByRole("option", { name: "Frame" }).getAttribute("aria-selected")).toBe("true");
  fireEvent.keyDown(search, { key: "Home" });
  expect(screen.getByRole("option", { name: "New" }).getAttribute("aria-selected")).toBe("true");

  fireEvent.keyDown(search, { key: "Enter" });
  await waitFor(() => expect(runNew).toHaveBeenCalledTimes(1));
  expect(onClose).toHaveBeenCalledTimes(1);
  expect(screen.getByRole("option", { name: /Save/ }).getAttribute("aria-disabled")).toBe("true");
});

test("keyboard selection follows the rendered category-grouped order", () => {
  render(
    <CommandPalette
      open
      onClose={() => {}}
      commands={[
        command({ id: "project-new", label: "New", category: "Project" }),
        command({ id: "view-frame", label: "Frame", category: "Viewport" }),
        command({ id: "project-open", label: "Open", category: "Project" }),
      ]}
    />,
  );
  const search = screen.getByRole("combobox", { name: "Search commands" });
  fireEvent.keyDown(search, { key: "ArrowDown" });
  // Project is one visual group, so Open follows New before the later Viewport group.
  expect(screen.getByRole("option", { name: "Open" }).getAttribute("aria-selected")).toBe("true");
});

test("disabled rows explain why, remain inert, and do not close the palette", () => {
  const execute = vi.fn();
  const onClose = vi.fn();
  render(
    <CommandPalette
      open
      onClose={onClose}
      commands={[command({ id: "save", label: "Save", disabled: true, disabledReason: "The project is read-only", execute })]}
    />,
  );
  const option = screen.getByRole("option", { name: /Save/ });
  expect(option.getAttribute("title")).toBe("The project is read-only");
  expect(option.hasAttribute("disabled")).toBe(true);
  fireEvent.click(option);
  expect(execute).not.toHaveBeenCalled();
  expect(onClose).not.toHaveBeenCalled();
});

test("async command errors stay in context, restore search focus, and notify the caller", async () => {
  const error = new Error("Project storage is unavailable");
  const onError = vi.fn();
  const onClose = vi.fn();
  render(
    <CommandPalette
      open
      onClose={onClose}
      onCommandError={onError}
      commands={[command({ id: "save", label: "Save", execute: () => Promise.reject(error) })]}
    />,
  );
  const search = screen.getByRole("combobox", { name: "Search commands" });
  fireEvent.keyDown(search, { key: "Enter" });

  const alert = await screen.findByRole("alert");
  expect(alert.textContent).toContain("Project storage is unavailable");
  expect(onError).toHaveBeenCalledWith(error, expect.objectContaining({ id: "save" }));
  expect(onClose).not.toHaveBeenCalled();
  expect(document.activeElement).toBe(search);
  await waitFor(() => expect(search.hasAttribute("disabled")).toBe(false));
});

test("Escape and the explicit close button dismiss the palette", () => {
  const onClose = vi.fn();
  const { rerender } = render(<CommandPalette open onClose={onClose} commands={COMMANDS} />);
  fireEvent.keyDown(window, { key: "Escape" });
  expect(onClose).toHaveBeenCalledTimes(1);

  rerender(<CommandPalette open onClose={onClose} commands={COMMANDS} />);
  fireEvent.click(screen.getByRole("button", { name: "Close command palette" }));
  expect(onClose).toHaveBeenCalledTimes(2);
});
