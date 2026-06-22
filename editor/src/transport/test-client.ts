//! A fully-stubbed `EditorClient` for component tests — override only the methods the test exercises.
//! Keeps the per-component tests robust as the client surface grows (one place to default new methods).
//! Imported only by `*.test.tsx`, so it's never in the production bundle.

import { vi } from "vitest";
import type { EditorClient } from "./session";

export function fakeClient(over: Partial<EditorClient> = {}): EditorClient {
  return {
    setField: vi.fn(() => "op"),
    bind: vi.fn(() => "op"),
    onEphemeral: () => () => {},
    revealTargets: () => Promise.resolve({ required: [], compatible: [], greyed: [], bound: [] }),
    describe: () => Promise.resolve({ created: null, kind: null, source: null, price: null, seam: null, balance: null }),
    walletInfo: () => Promise.resolve({ ok: true, balance: 100, cost: null, message: null }),
    topUp: () => Promise.resolve({ ok: true, balance: 200, cost: 100, message: null }),
    aiEdit: () => Promise.resolve({ ok: true, balance: 198, cost: 2, message: null }),
    undo: vi.fn(),
    entityActions: () => Promise.resolve([]),
    entityDetails: () => Promise.resolve(null),
    removeEntity: vi.fn(),
    duplicateEntity: () => Promise.resolve(null),
    focusEntity: vi.fn(),
    makeDynamic: () => Promise.resolve(true),
    catalog: () => Promise.resolve({}),
    catalogSearch: () => Promise.resolve({ items: [] }),
    addItem: vi.fn(() => Promise.resolve({ created: "e-new", balance: null, seam: null })),
    projectState: () => Promise.resolve({ path: null, dirty: false, recents: [], error: null }),
    newProject: vi.fn(() => Promise.resolve({ path: null, dirty: false, recents: [], error: null })),
    openProject: vi.fn(() => Promise.resolve({ path: "p.mtk", dirty: false, recents: ["p.mtk"], error: null })),
    saveProject: vi.fn(() => Promise.resolve({ path: "p.mtk", dirty: false, recents: ["p.mtk"], error: null })),
    saveProjectAs: vi.fn(() => Promise.resolve({ path: "p.mtk", dirty: false, recents: ["p.mtk"], error: null })),
    play: vi.fn(() => Promise.resolve({ playing: true, paused: false })),
    stop: vi.fn(() => Promise.resolve({ playing: false, paused: false })),
    pause: vi.fn(() => Promise.resolve({ playing: true, paused: true })),
    playState: () => Promise.resolve({ playing: false, paused: false }),
    ...over,
  };
}
