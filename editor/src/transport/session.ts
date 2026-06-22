//! The editor **session factory** (M10.1 / ADR-030) — the seam that makes the React `/editor` the
//! production shell UI. It picks the **real Tauri transport** (the packaged `.exe` talking to the live
//! `/core` over the `connect` Channel + the shell commands) when running inside the WebView, and falls
//! back to the in-process **MockCore** for `npm run dev` / Vitest. Either way the UI talks to one
//! [`EditorClient`] surface and the projection store is the single read-model (invariant 1): optimistic
//! echo on edit, reconcile on the authoritative `ProjectionDelta` (confirm/reject — every "no" explained,
//! ADR-010). The native viewport hot path never crosses this layer (invariant 4).

import { projectionStore } from "../store/projection";
import type {
  ActionItem,
  AddResponse,
  CatalogItem,
  CatalogSearch,
  DescribeResponse,
  EconResponse,
  EditIntent,
  EditTx,
  EntityDetails,
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
  /** Undo the last committed transaction (Ctrl-Z); the reverting delta streams back over the Channel. */
  undo(): void;
  /** The context-menu actions for an entity (M3.3) — each available-or-explained. */
  entityActions(id: string): Promise<ActionItem[]>;
  /** The hover-tooltip details for an entity (M3.3) — name + components + caps + bound. */
  entityDetails(id: string): Promise<EntityDetails | null>;
  /** Remove an entity + its edges (M3.3) — one undoable transaction (the delta streams back). */
  removeEntity(id: string): void;
  /** Duplicate an entity (M3.3) — one undoable transaction; resolves to the clone's id. */
  duplicateEntity(id: string): Promise<string | null>;
  /** Frame the camera on an entity (M3.3) — no mutation. */
  focusEntity(id: string): void;
  /** M8.3: turn a dead mesh into a correct dynamic body — one undoable transaction. */
  makeDynamic(id: string): Promise<boolean>;
  /** The browse catalog (M3.4 / ADR-019) — the ONE catalog (registry + marketplace + imported), grouped
   *  by category. The asset browser reuses this; it never forks the search/category logic. */
  catalog(): Promise<Record<string, CatalogItem[]>>;
  /** Search the one catalog (reuses the tiered resolver) — ranked matches + a no-match seam. */
  catalogSearch(query: string): Promise<CatalogSearch>;
  /** Instantiate a catalog item into the scene (place-into-scene) — one undoable, persisted entity. */
  addItem(id: string, source: string): Promise<AddResponse>;
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

  undo(): void {
    void this.core.invoke("undo").catch((e: unknown) => console.error("undo failed", e));
  }

  entityActions(id: string): Promise<ActionItem[]> {
    return this.core.invoke<ActionItem[]>("entity_actions", { id });
  }
  entityDetails(id: string): Promise<EntityDetails | null> {
    return this.core.invoke<EntityDetails | null>("entity_details", { id });
  }
  removeEntity(id: string): void {
    void this.core.invoke("remove_entity", { id }).catch((e: unknown) => console.error("remove_entity failed", e));
  }
  duplicateEntity(id: string): Promise<string | null> {
    return this.core.invoke<string | null>("duplicate_entity", { id });
  }
  focusEntity(id: string): void {
    void this.core.invoke("focus_entity", { id }).catch((e: unknown) => console.error("focus_entity failed", e));
  }
  makeDynamic(id: string): Promise<boolean> {
    return this.core.invoke<boolean>("make_dynamic", { id });
  }

  catalog(): Promise<Record<string, CatalogItem[]>> {
    return this.core.invoke<Record<string, CatalogItem[]>>("catalog");
  }
  catalogSearch(query: string): Promise<CatalogSearch> {
    return this.core.invoke<CatalogSearch>("catalog_search", { query });
  }
  addItem(id: string, source: string): Promise<AddResponse> {
    return this.core.invoke<AddResponse>("add_item", { id, source });
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
  undo(): void {
    /* the dev MockCore has no undo stack — a no-op (the real shell undoes over the Channel) */
  }
  entityActions(id: string): Promise<ActionItem[]> {
    const e = projectionStore.getState().displayed[id];
    const canBind = !!e?.components.Socket;
    return Promise.resolve([
      { action: "bind", label: "Bind…", available: canBind, reason: canBind ? undefined : "no unmet requirement to bind", mutates: false },
      { action: "remove", label: "Remove", available: true, mutates: true },
      { action: "duplicate", label: "Duplicate", available: true, mutates: true },
      { action: "focus", label: "Focus", available: true, mutates: false },
      { action: "inspect", label: "Inspect", available: true, mutates: false },
    ]);
  }
  entityDetails(id: string): Promise<EntityDetails | null> {
    const e = projectionStore.getState().displayed[id];
    if (!e) return Promise.resolve(null);
    const c = e.components;
    return Promise.resolve({
      id,
      name: e.name,
      components: Object.keys(c),
      provides: c.Provides?.capability != null ? [String(c.Provides.capability)] : [],
      requires: c.Socket?.accepts != null ? [String(c.Socket.accepts)] : [],
      boundTo: [],
    });
  }
  removeEntity(_id: string): void {}
  duplicateEntity(_id: string): Promise<string | null> {
    return Promise.resolve(null);
  }
  focusEntity(_id: string): void {}
  makeDynamic(_id: string): Promise<boolean> {
    return Promise.resolve(true);
  }
  catalog(): Promise<Record<string, CatalogItem[]>> {
    const item = (id: string, label: string, category: string, source: string): CatalogItem => ({
      id, label, bucket: category, category, source, provides: [], requires: [],
    });
    return Promise.resolve({
      Health: [item("HealthBar", "HealthBar", "Health", "local")],
      UI: [item("Button", "Button", "UI", "local")],
    });
  }
  catalogSearch(query: string): Promise<CatalogSearch> {
    return this.catalog().then((groups) => {
      const all = Object.values(groups).flat();
      const items = all.filter((i) => i.label.toLowerCase().includes(query.toLowerCase()));
      return { items, seam: items.length === 0 ? "generate" : undefined };
    });
  }
  addItem(_id: string, _source: string): Promise<AddResponse> {
    return Promise.resolve({ created: "e-new", balance: null, seam: null });
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
