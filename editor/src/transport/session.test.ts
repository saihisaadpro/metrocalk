//! Transport-level provenance hint (M11.5 / ADR-044, SA-34) — verified headless: after an import the real
//! `TauriClient` reads the `asset_provenance` projection and, when the just-imported asset perceptually
//! matches an already-loaded one (a near-duplicate the exact content-hash dedup misses), surfaces a single
//! lightweight toast — never a silent merge, never a blocked import. A hint failure must not break the import.

import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { TauriClient } from "./session";
import { toastStore } from "../store/toasts";

// A fake Tauri core: a stub Channel (the constructor opens the projection channel) + a scripted `invoke`.
function clientWith(invoke: (cmd: string) => Promise<unknown>): TauriClient {
  const core = {
    invoke: vi.fn(invoke),
    Channel: class {
      onmessage: (m: unknown) => void = () => {};
    },
  };
  return new TauriClient(core as unknown as ConstructorParameters<typeof TauriClient>[0]);
}

const texts = (): string[] => toastStore.getState().toasts.map((t) => t.text);

beforeEach(() => toastStore.getState().reset());
afterEach(() => vi.restoreAllMocks());

test("import surfaces ONE near-duplicate toast when the provenance projection reports a perceptual match", async () => {
  const client = clientWith((cmd) => {
    if (cmd === "import_asset") return Promise.resolve("ent-7");
    if (cmd === "asset_provenance")
      return Promise.resolve({ kind: "imported", source: "wide.glb", nearDuplicateOf: "ripple_quad.glb" });
    return Promise.resolve(null);
  });

  const id = await client.importAsset("/assets/wide.glb");

  expect(id).toBe("ent-7"); // the import contract is unchanged — callers still get the entity id
  const t = toastStore.getState().toasts;
  expect(t).toHaveLength(1);
  expect(t[0].kind).toBe("info");
  expect(t[0].text).toContain("ripple_quad.glb");
  expect(t[0].text).toMatch(/near-duplicate/i);
});

test("import surfaces NO toast when the asset is not a near-duplicate", async () => {
  const client = clientWith((cmd) => {
    if (cmd === "import_asset_dialog") return Promise.resolve("ent-8");
    if (cmd === "asset_provenance")
      return Promise.resolve({ kind: "imported", source: "unique.glb", nearDuplicateOf: null });
    return Promise.resolve(null);
  });

  const id = await client.importAssetDialog();

  expect(id).toBe("ent-8");
  expect(texts()).toEqual([]);
});

test("a cancelled import (null id) queries no provenance and posts no toast", async () => {
  const invoked: string[] = [];
  const client = clientWith((cmd) => {
    invoked.push(cmd);
    if (cmd === "import_asset_dialog") return Promise.resolve(null);
    return Promise.resolve(null);
  });

  const id = await client.importAssetDialog();

  expect(id).toBeNull();
  expect(texts()).toEqual([]);
  expect(invoked).not.toContain("asset_provenance");
});

test("a provenance-hint failure never breaks the import (best-effort)", async () => {
  vi.spyOn(console, "error").mockImplementation(() => {});
  const client = clientWith((cmd) => {
    if (cmd === "import_asset") return Promise.resolve("ent-9");
    if (cmd === "asset_provenance") return Promise.reject(new Error("projection unavailable"));
    return Promise.resolve(null);
  });

  await expect(client.importAsset("/x.glb")).resolves.toBe("ent-9");
  expect(texts()).toEqual([]);
});
