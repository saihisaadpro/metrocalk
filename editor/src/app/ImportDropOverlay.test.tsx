import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { toastStore } from "../store/toasts";
import { setStatus, uiStore } from "../store/ui";
import { ImportDropOverlay, type ImportDropOverlayProps } from "./ImportDropOverlay";
import {
  NATIVE_IMPORT_LIFECYCLE_EVENT,
  type NativeImportLifecycleEvent,
} from "./nativeImportLifecycle";

interface TestEventEnvelope {
  event: string;
  id: number;
  payload: NativeImportLifecycleEvent;
}

type TestEventHandler = (event: TestEventEnvelope) => void;

const tauriGlobal = globalThis as unknown as { __TAURI__?: unknown };

describe("native import drop overlay", () => {
  let listen: ReturnType<typeof vi.fn>;
  let invoke: ReturnType<typeof vi.fn>;
  let actions: {
    [Key in keyof ImportDropOverlayProps]: ReturnType<typeof vi.fn>;
  };

  beforeEach(() => {
    listen = vi.fn(async () => vi.fn());
    invoke = vi.fn(async () => true);
    tauriGlobal.__TAURI__ = { event: { listen }, core: { invoke } };
    actions = {
      onEntityImported: vi.fn(),
      onOpenImportReport: vi.fn(),
      onOpenFormats: vi.fn(),
      onImportAnother: vi.fn(),
    };
  });

  afterEach(() => {
    delete tauriGlobal.__TAURI__;
    toastStore.getState().reset();
    setStatus("");
  });

  async function mount() {
    render(<ImportDropOverlay {...actions} />);
    await waitFor(() => expect(listen).toHaveBeenCalledWith(NATIVE_IMPORT_LIFECYCLE_EVENT, expect.any(Function)));
  }

  function emit(payload: NativeImportLifecycleEvent) {
    const handler = listen.mock.calls[0]?.[1] as TestEventHandler;
    act(() => handler({ event: NATIVE_IMPORT_LIFECYCLE_EVENT, id: 1, payload }));
  }

  it("shows supported and unavailable files while hovering, then clears on leave", async () => {
    await mount();
    emit({
      phase: "hovered",
      files: [
        { name: "skid.step", supported: true },
        { name: "notes.txt", supported: false, reason: "Unsupported format" },
      ],
    });

    const overlay = screen.getByTestId("native-import-overlay");
    expect(overlay.getAttribute("data-phase")).toBe("hovered");
    expect(overlay.textContent).toContain("Release to import");
    expect(overlay.textContent).toContain("Ready");
    expect(overlay.textContent).toContain("skid.step");
    expect(overlay.textContent).toContain("Supported");
    expect(overlay.textContent).toContain("Unsupported format");

    emit({ phase: "left" });
    expect(screen.queryByTestId("native-import-overlay")).toBeNull();

    emit({ phase: "hovered", files: [{ name: "notes.txt", supported: false }] });
    expect(screen.getByTestId("native-import-overlay").textContent).toContain("This drop cannot be imported");
    expect(screen.getByTestId("native-import-overlay").textContent).toContain("Unavailable");
  });

  it("projects queued and delayed progress, then truthfully stops at a confirmed safe checkpoint", async () => {
    await mount();
    emit({
      phase: "dropped",
      batchId: 7,
      files: [{ name: "plant.3dxml", supported: true }],
    });
    expect(screen.getByTestId("native-import-overlay").getAttribute("aria-busy")).toBe("true");
    expect(uiStore.getState().status).toBe("1 file queued for import");

    emit({
      phase: "progress",
      batchId: 7,
      index: 2,
      total: 3,
      completed: 1,
      fileName: "plant.3dxml",
      stage: "delayed",
      elapsedMs: 12_600,
    });

    const overlay = screen.getByTestId("native-import-overlay");
    expect(overlay.textContent).toContain("Still importing plant.3dxml · 13s elapsed");
    expect(overlay.textContent).toContain("Still working");
    expect(overlay.textContent).toContain("File 2 of 3 · 1 complete");
    expect(overlay.textContent).toContain("The importer is still working in order");
    expect(screen.getByRole("progressbar", { name: "Import progress for plant.3dxml" })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Cancel import" }));
    expect(invoke).toHaveBeenCalledWith("cancel_native_import", { batchId: 7 });
    expect((screen.getByRole("button", { name: "Stopping import" }) as HTMLButtonElement).disabled).toBe(true);
    expect(overlay.textContent).toContain("Stopping at a safe checkpoint");
    expect(overlay.textContent).toContain("Parsing may finish");
    expect(overlay.textContent).toContain("no scene changes will be committed");

    emit({
      phase: "cancelled",
      batchId: 7,
      index: 2,
      total: 3,
      fileName: "plant.3dxml",
      message: "Import of plant.3dxml stopped at a safe checkpoint. No scene changes were committed.",
    });
    expect(screen.getByTestId("native-import-overlay").getAttribute("aria-busy")).toBeNull();
    expect(screen.getByTestId("native-import-overlay").textContent).toContain("Cancelled");
    expect(screen.getByTestId("native-import-overlay").textContent).toContain("safely retry");
    expect(uiStore.getState().status).toContain("No scene changes were committed");
    expect(toastStore.getState().toasts.at(-1)).toMatchObject({ kind: "info" });
    expect(screen.queryByRole("button", { name: /cancel import/i })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Import another…" }));
    expect(actions.onImportAnother).toHaveBeenCalledOnce();
  });

  it("re-opens Cancel when Rust reports that the final checkpoint already passed", async () => {
    invoke.mockResolvedValueOnce(false);
    await mount();
    emit({
      phase: "progress",
      batchId: 18,
      index: 1,
      total: 1,
      completed: 0,
      fileName: "nearly-done.step",
      stage: "importing",
      elapsedMs: 10,
    });

    fireEvent.click(screen.getByRole("button", { name: "Cancel import" }));
    await waitFor(() => expect((screen.getByRole("button", { name: "Cancel import" }) as HTMLButtonElement).disabled).toBe(false));
    expect(uiStore.getState().status).toContain("already crossed its final commit checkpoint");
  });

  it("selects an imported entity, opens its report, and dismisses the terminal card", async () => {
    await mount();
    emit({
      phase: "succeeded",
      batchId: 8,
      index: 1,
      total: 1,
      fileName: "skid.step",
      subject: { kind: "entity", rootId: "root-8" },
      message: "Imported skid.step",
    });

    expect(actions.onEntityImported).toHaveBeenCalledOnce();
    expect(actions.onEntityImported).toHaveBeenCalledWith("root-8");
    expect(toastStore.getState().toasts.at(-1)).toMatchObject({ text: "Imported skid.step", kind: "success" });
    fireEvent.click(screen.getByRole("button", { name: "Import report" }));
    expect(actions.onOpenImportReport).toHaveBeenCalledOnce();
    fireEvent.click(screen.getByRole("button", { name: "Dismiss" }));
    expect(screen.queryByTestId("native-import-overlay")).toBeNull();
  });

  it("offers recovery for refusals and failures and lets either state be dismissed", async () => {
    await mount();
    emit({
      phase: "refused",
      batchId: 9,
      index: 1,
      total: 1,
      fileName: "notes.txt",
      message: "notes.txt is not supported",
      recoverable: true,
    });

    expect(screen.getByRole("alert").textContent).toContain("No scene changes were made");
    fireEvent.click(screen.getByRole("button", { name: "Supported formats" }));
    fireEvent.click(screen.getByRole("button", { name: "Import another…" }));
    expect(actions.onOpenFormats).toHaveBeenCalledOnce();
    expect(actions.onImportAnother).toHaveBeenCalledOnce();
    fireEvent.click(screen.getByRole("button", { name: "Dismiss" }));
    expect(screen.queryByTestId("native-import-overlay")).toBeNull();

    emit({
      phase: "failed",
      batchId: 10,
      index: 1,
      total: 1,
      fileName: "broken.step",
      message: "Could not read broken.step",
      recoverable: true,
    });
    expect(screen.getByRole("alert").textContent).toContain("The editor remains usable");
    fireEvent.click(screen.getByRole("button", { name: "Import report" }));
    fireEvent.click(screen.getByRole("button", { name: "Import another…" }));
    expect(actions.onOpenImportReport).toHaveBeenCalledOnce();
    expect(actions.onImportAnother).toHaveBeenCalledTimes(2);
    fireEvent.click(screen.getByRole("button", { name: "Dismiss" }));
    expect(screen.queryByTestId("native-import-overlay")).toBeNull();
  });

  it("ignores late terminal events from an older batch, including their selection side effects", async () => {
    await mount();
    emit({
      phase: "progress",
      batchId: 22,
      index: 1,
      total: 1,
      completed: 0,
      fileName: "new.step",
      stage: "importing",
      elapsedMs: 300,
    });
    emit({
      phase: "succeeded",
      batchId: 21,
      index: 1,
      total: 1,
      fileName: "old.step",
      subject: { kind: "entity", rootId: "old-root" },
      message: "Imported old.step",
    });

    expect(screen.getByTestId("native-import-overlay").textContent).toContain("Importing new.step");
    expect(actions.onEntityImported).not.toHaveBeenCalled();
    expect(toastStore.getState().toasts).toHaveLength(0);
  });
});
