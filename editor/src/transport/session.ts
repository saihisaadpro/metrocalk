//! The editor **session factory** (M10.1 / ADR-030) — the seam that makes the React `/editor` the
//! production shell UI. It picks the **real Tauri transport** (the packaged `.exe` talking to the live
//! `/core` over the `connect` Channel + the shell commands) when running inside the WebView, and falls
//! back to the in-process **MockCore** for `npm run dev` / Vitest. Either way the UI talks to one
//! [`EditorClient`] surface and the projection store is the single read-model (invariant 1): optimistic
//! echo on edit, reconcile on the authoritative `ProjectionDelta` (confirm/reject — every "no" explained,
//! ADR-010). The native viewport hot path never crosses this layer (invariant 4).

import { projectionStore } from "../store/projection";
import type {
  DescribeResponse,
  EconResponse,
  EditIntent,
  EditTx,
  EntityProjection,
  Json,
  JsonPatch,
  ProjectionDelta,
  RevealResponse,
} from "./protocol";
import { DeltaClient } from "./client";
import { MockCore } from "./mock-core";
import { inProcessPair } from "./transport";

/** The one client surface the React UI talks to (the real shell transport + the dev MockCore both satisfy it). */
export interface EditorClient {
  /** Optimistic field edit → a JSON-Patch `EditTx` (the same language the AI layer emits). */
  setField(id: string, component: string, field: string, value: Json): string;
  /** Optimistic bind-by-intent; the authoritative edge streams back over the Channel. */
  bind(from: string, rel: string, to: string): string;
  /** Subscribe to ephemeral (preview/presence) frames — a no-op on the desktop shell for now. */
  onEphemeral(cb: (data: Json) => void): () => void;
  /** Reveal the ranked compatible bind targets (+ greyed-with-reason, + bound) for an entity (north-star #1). */
  revealTargets(id: string): Promise<RevealResponse>;
  /** Describe-to-create: resolve a free-text query (local → marketplace → generate seam). */
  describe(query: string): Promise<DescribeResponse>;
  /** The user's token balance (M7). */
  walletInfo(): Promise<EconResponse>;
  /** Sandbox top-up (M7 — no real money, ADR-004/018). */
  topUp(): Promise<EconResponse>;
  /** AI-edit "make it rustier" on an entity (M7 — schema-validated patch, debit-on-success). */
  aiEdit(id: string): Promise<EconResponse>;
}

// ── the Tauri global (withGlobalTauri: true exposes window.__TAURI__.core; no @tauri-apps/api dep) ──────
interface TauriChannel<T> {
  onmessage: (msg: T) => void;
}
interface TauriCore {
  invoke<T = unknown>(cmd: string, args?: Record<string, unknown>): Promise<T>;
  Channel: new <T>() => TauriChannel<T>;
}
function tauriCore(): TauriCore | null {
  const w = globalThis as unknown as { __TAURI__?: { core?: TauriCore } };
  return w.__TAURI__?.core ?? null;
}

/** True when running inside the packaged Tauri WebView (vs `npm run dev` / Vitest). */
export const isTauri = (): boolean => tauriCore() !== null;

/** The REAL shell transport: `connect` streams committed `ProjectionDelta`s into the store; edits go out
 *  through the shell's commands (`submit_edit` / `bind_target`) — the exact contract the vanilla scaffold
 *  used, so the 61 commands + the Channel are unchanged (M10.1 swaps the UI, not the core). */
class TauriClient implements EditorClient {
  private opCounter = 0;
  private readonly core: TauriCore;

  constructor(core: TauriCore) {
    this.core = core;
    const channel = new core.Channel<ProjectionDelta>();
    channel.onmessage = (delta) => projectionStore.getState().applyDelta(delta);
    void this.core
      .invoke("connect", { channel })
      .catch((e: unknown) => console.error("connect failed", e));
  }

  private nextOp(): string {
    this.opCounter += 1;
    return `op-${this.opCounter}`;
  }

  setField(id: string, component: string, field: string, value: Json): string {
    const clientOpId = this.nextOp();
    const intent: EditIntent = { kind: "setField", id, component, field, value };
    const patches: JsonPatch[] = [
      { op: "replace", path: `/entities/${id}/components/${component}/${field}`, value },
    ];
    projectionStore.getState().optimisticEdit({ clientOpId, intent });
    const tx: EditTx = { clientOpId, label: `set ${component}.${field}`, patches, intent };
    void this.core.invoke("submit_edit", { tx }).catch((e: unknown) => console.error("submit_edit failed", e));
    return clientOpId;
  }

  bind(from: string, rel: string, to: string): string {
    const clientOpId = this.nextOp();
    const intent: EditIntent = { kind: "bind", from, rel, to };
    projectionStore.getState().optimisticEdit({ clientOpId, intent });
    // The shell's dedicated M3.1 bind command; the authoritative addEdge streams back over the Channel.
    void this.core.invoke("bind_target", { from, to }).catch((e: unknown) => console.error("bind_target failed", e));
    return clientOpId;
  }

  onEphemeral(): () => void {
    return () => {};
  }

  revealTargets(id: string): Promise<RevealResponse> {
    return this.core.invoke<RevealResponse>("reveal_targets", { id });
  }

  describe(query: string): Promise<DescribeResponse> {
    return this.core.invoke<DescribeResponse>("describe", { query });
  }

  walletInfo(): Promise<EconResponse> {
    return this.core.invoke<EconResponse>("wallet_info");
  }

  topUp(): Promise<EconResponse> {
    return this.core.invoke<EconResponse>("top_up");
  }

  aiEdit(id: string): Promise<EconResponse> {
    return this.core.invoke<EconResponse>("ai_edit", { id });
  }
}

// ── dev / test transport: the in-process MockCore + the framed DeltaClient (the unchanged M2.5 path) ────
const CAPS = ["Health", "Shield", "Click", "Damage", "Light"];

/** A seeded 5k scene for the MockCore (deterministic so the dev view is reproducible). */
function buildWorld(n: number): EntityProjection[] {
  const out: EntityProjection[] = [];
  let seed = 0x9e3779b9;
  const rnd = () => ((seed = (seed * 1664525 + 1013904223) >>> 0) / 0xffffffff);
  for (let i = 0; i < n; i++) {
    const components: EntityProjection["components"] = {
      Transform: { x: Math.round(rnd() * 100), y: Math.round(rnd() * 100), z: 0 },
    };
    if (i % 7 === 0) components.Material = { color: "#88ccff", metalness: 0.2 };
    if (i % 5 === 0) components.Provides = { capability: CAPS[i % CAPS.length] };
    if (i % 11 === 0) components.Socket = { accepts: CAPS[(i + 1) % CAPS.length] };
    if (i % 13 === 0) components.Targeting = { target: "" };
    out.push({ id: `e${i}`, name: `Entity ${i}`, parentId: i === 0 ? null : "e0", components });
  }
  return out;
}

/** The dev/test client: a framed `DeltaClient` for edits + minimal store-derived query mocks so
 *  `npm run dev` still renders the reveal/describe surfaces without a live core. (Vitest tests inject
 *  their own stubbed `EditorClient`; the real reveal/describe come from the shell commands under Tauri.) */
class MockClient implements EditorClient {
  private balance = 100;
  constructor(private readonly inner: DeltaClient) {}
  setField(id: string, component: string, field: string, value: Json): string {
    return this.inner.setField(id, component, field, value);
  }
  bind(from: string, rel: string, to: string): string {
    return this.inner.bind(from, rel, to);
  }
  onEphemeral(cb: (data: Json) => void): () => void {
    return this.inner.onEphemeral(cb);
  }
  revealTargets(id: string): Promise<RevealResponse> {
    const s = projectionStore.getState();
    const sel = s.displayed[id];
    const need = (sel?.components.Socket?.accepts as string | undefined) ?? null;
    const providers = s.order
      .map((eid) => s.displayed[eid])
      .filter((e): e is EntityProjection => !!e && e.id !== id && "Provides" in e.components);
    const compatible = providers
      .filter((e) => !need || e.components.Provides.capability === need)
      .slice(0, 8)
      .map((e, i) => ({ id: e.id, name: e.name, distance: i, affinity: 100 - i }));
    const greyed = providers
      .filter((e) => need && e.components.Provides.capability !== need)
      .slice(0, 3)
      .map((e) => ({ id: e.id, name: e.name, reason: `doesn't provide ${need}` }));
    return Promise.resolve({ required: need ? [need] : [], compatible, greyed, bound: [] });
  }
  describe(_query: string): Promise<DescribeResponse> {
    return Promise.resolve({ created: null, kind: null, source: null, price: null, seam: "generate", balance: null });
  }
  walletInfo(): Promise<EconResponse> {
    return Promise.resolve({ ok: true, balance: this.balance, cost: null, message: null });
  }
  topUp(): Promise<EconResponse> {
    this.balance += 100;
    return Promise.resolve({ ok: true, balance: this.balance, cost: 100, message: null });
  }
  aiEdit(_id: string): Promise<EconResponse> {
    if (this.balance < 2) {
      return Promise.resolve({ ok: false, balance: this.balance, cost: null, message: "insufficient balance" });
    }
    this.balance -= 2;
    return Promise.resolve({ ok: true, balance: this.balance, cost: 2, message: null });
  }
}

function mockSession(): EditorClient {
  const [uiT, coreT] = inProcessPair();
  const core = new MockCore(coreT, buildWorld(5000));
  const client = new DeltaClient(uiT);
  core.emitScene();
  return new MockClient(client);
}

/** Build the editor session: the real Tauri shell transport inside the WebView, else the dev MockCore. */
export function createSession(): EditorClient {
  const core = tauriCore();
  return core ? new TauriClient(core) : mockSession();
}
