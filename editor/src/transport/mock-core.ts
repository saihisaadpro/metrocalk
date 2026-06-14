//! `MockCore` — stands in for the authoritative WASM/Rust core in the scaffold and tests. It receives
//! `EditTx` frames, validates them **semantically** (the stand-in for the ECS compatibility query +
//! merge-validation), applies accepted edits to its own authoritative projection, and emits a `%LOR`
//! `ProjectionDelta` that **confirms** (drop the optimistic op) or **rejects** (revert it + a reason,
//! the north-star "every 'no' explained"). M2.6 swaps this for the real core — the wire and the
//! `ProjectionDelta`/`EditTx` contract are unchanged.

import { decode, decodeDocUpdate, encodeDocUpdate, encodeEphemeral, FrameKind } from "./frame";
import { decodeJson, encodeJson } from "./protocol";
import type { EditTx, EntityProjection, ProjectionDelta, ProjectionOp, RejectInfo } from "./protocol";
import type { DeltaTransport } from "./transport";

const ROOM = 1;

export interface MockCoreOptions {
  /** Stand-in for the ECS compatibility query: may entity `from` bind to `to` via `rel`? */
  canBind?: (from: EntityProjection, to: EntityProjection, rel: string) => { ok: true } | { ok: false; reason: string };
  /** Defer outgoing deltas into a queue (call [`flush`](MockCore.flush) to release) so a test can
   *  observe the optimistic state before reconciliation. Real cores are async; the in-process
   *  loopback is synchronous, so this models the round-trip gap. */
  defer?: boolean;
}

export class MockCore {
  private base: Record<string, EntityProjection> = {};
  private batch = 1n;
  private canBind: NonNullable<MockCoreOptions["canBind"]>;
  private defer: boolean;
  private queue: Uint8Array[] = [];

  constructor(
    private transport: DeltaTransport,
    seed: EntityProjection[],
    opts: MockCoreOptions = {},
  ) {
    for (const e of seed) this.base[e.id] = e;
    this.defer = opts.defer ?? false;
    this.canBind =
      opts.canBind ??
      ((from, to, _rel) =>
        // default rule: a "Socket" component on the target advertises what it accepts; reject otherwise
        to.components["Socket"]?.["accepts"] === from.components["Provides"]?.["capability"]
          ? { ok: true }
          : {
              ok: false,
              reason: `'${from.name}' provides '${String(from.components["Provides"]?.["capability"] ?? "—")}', which '${to.name}' does not accept`,
            });
    transport.setOnRecv((f) => this.onFrame(f));
  }

  /** Send a server-initiated committed delta (e.g. the initial scene load). */
  push(ops: ProjectionOp[]): void {
    this.applyOpsLocally(ops);
    this.emit({ ops });
  }

  /** Emit the full authoritative scene as one committed delta (the initial load the UI projects). */
  emitScene(): void {
    const ops: ProjectionOp[] = [];
    for (const e of Object.values(this.base)) {
      ops.push({ op: "upsert", id: e.id, name: e.name, parentId: e.parentId });
      for (const [c, fields] of Object.entries(e.components)) {
        for (const [f, v] of Object.entries(fields)) {
          ops.push({ op: "setField", id: e.id, component: c, field: f, value: v });
        }
      }
    }
    this.emit({ ops });
  }

  /** Push an ephemeral (`%EPH`) frame — live preview / presence (opaque bytes; JSON here). */
  pushEphemeral(data: unknown): void {
    this.transport.send(encodeEphemeral(ROOM, encodeJson(data)));
  }

  private onFrame(frame: Uint8Array): void {
    const h = decode(frame);
    if (h.kind !== FrameKind.DocUpdate) return; // handshake/ack/ping handled elsewhere
    const du = decodeDocUpdate(h.payload);
    for (const blob of du.blobs) {
      const tx = decodeJson<EditTx>(blob);
      if (!("patches" in tx)) continue;
      this.handleEdit(tx);
    }
  }

  private handleEdit(tx: EditTx): void {
    const intent = tx.intent;
    if (intent.kind === "setField") {
      const ent = this.base[intent.id];
      if (!ent) {
        this.emit({ ops: [], rejects: [{ clientOpId: tx.clientOpId, reason: `unknown entity '${intent.id}'` }] });
        return;
      }
      const ops: ProjectionOp[] = [
        { op: "setField", id: intent.id, component: intent.component, field: intent.field, value: intent.value },
      ];
      this.applyOpsLocally(ops);
      this.emit({ ops, confirms: [tx.clientOpId] });
      return;
    }
    if (intent.kind === "bind") {
      const from = this.base[intent.from];
      const to = this.base[intent.to];
      if (!from || !to) {
        this.emit({ ops: [], rejects: [{ clientOpId: tx.clientOpId, reason: `bind references an unknown entity` }] });
        return;
      }
      const verdict = this.canBind(from, to, intent.rel);
      if (!verdict.ok) {
        const rejects: RejectInfo[] = [{ clientOpId: tx.clientOpId, reason: verdict.reason }];
        this.emit({ ops: [], rejects });
        return;
      }
      const ops: ProjectionOp[] = [{ op: "addEdge", from: intent.from, rel: intent.rel, to: intent.to }];
      this.emit({ ops, confirms: [tx.clientOpId] });
      return;
    }
  }

  private applyOpsLocally(ops: ProjectionOp[]): void {
    for (const op of ops) {
      if (op.op === "upsert") {
        const prev = this.base[op.id];
        this.base[op.id] = {
          id: op.id,
          name: op.name ?? prev?.name ?? op.id,
          parentId: op.parentId !== undefined ? op.parentId : (prev?.parentId ?? null),
          components: prev?.components ?? {},
        };
      } else if (op.op === "setField") {
        const e = this.base[op.id];
        if (e) (e.components[op.component] ??= {})[op.field] = op.value;
      }
    }
  }

  private emit(delta: ProjectionDelta): void {
    const frame = encodeDocUpdate(ROOM, this.batch++, [encodeJson(delta)]);
    if (this.defer) this.queue.push(frame);
    else this.transport.send(frame);
  }

  /** Release deferred deltas (only meaningful with `defer: true`). */
  flush(): void {
    const q = this.queue;
    this.queue = [];
    for (const f of q) this.transport.send(f);
  }
}
