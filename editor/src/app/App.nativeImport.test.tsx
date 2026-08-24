import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { projectionStore } from "../store/projection";
import { toastStore } from "../store/toasts";
import { App } from "./App";
import {
  NATIVE_IMPORT_LIFECYCLE_EVENT,
  type NativeImportLifecycleEvent,
} from "./nativeImportLifecycle";

interface TestEventEnvelope {
  event: string;
  id: number;
  payload: NativeImportLifecycleEvent;
}

const tauriGlobal = globalThis as unknown as { __TAURI__?: unknown };

afterEach(() => {
  delete tauriGlobal.__TAURI__;
  act(() => {
    projectionStore.getState().reset();
    toastStore.getState().reset();
  });
  window.localStorage.removeItem("metrocalk:shell-layout:v1:left-collapsed");
  window.localStorage.removeItem("metrocalk:shell-layout:v1:right-collapsed");
  window.localStorage.removeItem("metrocalk:shell-layout:v1:tool-rail-minimized");
});

describe("App native import projection", () => {
  it("mounts the lifecycle overlay on the stage and routes success and report actions through the shell", async () => {
    const listen = vi.fn(async (_event: string, _handler: (event: TestEventEnvelope) => void) => vi.fn());
    tauriGlobal.__TAURI__ = { event: { listen } };
    render(<App />);
    await waitFor(() => expect(listen).toHaveBeenCalledWith(NATIVE_IMPORT_LIFECYCLE_EVENT, expect.any(Function)));

    const handler = listen.mock.calls[0][1] as (event: TestEventEnvelope) => void;
    await act(async () => {
      handler({
        event: NATIVE_IMPORT_LIFECYCLE_EVENT,
        id: 1,
        payload: {
          phase: "succeeded",
          batchId: 31,
          index: 1,
          total: 1,
          fileName: "line.step",
          subject: { kind: "entity", rootId: "imported-root" },
          message: "Imported line.step",
        },
      });
      await new Promise((resolve) => window.setTimeout(resolve, 0));
    });

    expect(projectionStore.getState().selectedId).toBe("imported-root");
    expect(screen.getByTestId("engine-model").getAttribute("aria-selected")).toBe("true");
    expect(screen.getByTestId("bottom-dock").className).toContain("is-open");
    fireEvent.click(screen.getByRole("button", { name: "Import report" }));
    expect(document.getElementById("bottom-workspaces-import-tab")?.getAttribute("aria-selected")).toBe("true");
  });
});
