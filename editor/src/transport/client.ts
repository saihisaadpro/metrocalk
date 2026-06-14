//! `DeltaClient` — the UI side of the wire. Wires a [`DeltaTransport`] to the projection store:
//! decodes `%LOR` committed deltas → applies to the store + acks; routes `%EPH` ephemeral frames to a
//! separate live-preview/presence path; and sends edit transactions, applying them **optimistically**
//! first so the UI shows instantly and reconciles on the authoritative echo.
//!
//! Edit transactions (UI→core) and committed deltas (core→UI) both ride `%LOR` in this scaffold,
//! disambiguated by payload shape; in production edits route via the `invoke`/commit path while
//! deltas stream over the Channel — the `EditTx`/`ProjectionDelta` contract is identical either way.

import { decode, decodeAck, decodeDocUpdate, encodeAck, encodeDocUpdate, encodeHandshake, FrameKind } from "./frame";
import { decodeJson, encodeJson } from "./protocol";
import type { EditIntent, EditTx, Json, JsonPatch, ProjectionDelta } from "./protocol";
import { projectionStore } from "../store/projection";
import type { DeltaTransport } from "./transport";

const ROOM = 1;
let opCounter = 0;
const nextOpId = (): string => `op-${++opCounter}`;

/** True for a UI→core edit payload (vs a core→UI `ProjectionDelta`). */
function isEditTx(v: unknown): v is EditTx {
  return typeof v === "object" && v !== null && "patches" in v && "clientOpId" in v;
}

export class DeltaClient {
  private batch = 1n;
  private ephemeralListeners = new Set<(data: Json) => void>();

  constructor(
    private transport: DeltaTransport,
    peerId = 0xa11ce,
  ) {
    transport.setOnRecv((frame) => this.onFrame(frame));
    transport.send(encodeHandshake(ROOM, BigInt(peerId), new Uint8Array(0)));
  }

  /** Optimistic field edit → JSON-Patch transaction (the same language the AI layer emits). */
  setField(id: string, component: string, field: string, value: Json): string {
    const clientOpId = nextOpId();
    const intent: EditIntent = { kind: "setField", id, component, field, value };
    const patches: JsonPatch[] = [
      { op: "replace", path: `/entities/${id}/components/${component}/${field}`, value },
    ];
    projectionStore.getState().optimisticEdit({ clientOpId, intent });
    this.sendEdit({ clientOpId, label: `set ${component}.${field}`, patches, intent });
    return clientOpId;
  }

  /** Optimistic bind → JSON-Patch transaction. A rejected bind is reverted + its reason surfaced. */
  bind(from: string, rel: string, to: string): string {
    const clientOpId = nextOpId();
    const intent: EditIntent = { kind: "bind", from, rel, to };
    const patches: JsonPatch[] = [{ op: "add", path: `/edges/-`, value: `${from}|${rel}|${to}` }];
    projectionStore.getState().optimisticEdit({ clientOpId, intent });
    this.sendEdit({ clientOpId, label: `bind ${rel}`, patches, intent });
    return clientOpId;
  }

  /** Subscribe to ephemeral (`%EPH`) frames — live preview / presence, NOT committed state. */
  onEphemeral(cb: (data: Json) => void): () => void {
    this.ephemeralListeners.add(cb);
    return () => this.ephemeralListeners.delete(cb);
  }

  private sendEdit(tx: EditTx): void {
    this.transport.send(encodeDocUpdate(ROOM, this.batch++, [encodeJson(tx)]));
  }

  private onFrame(frame: Uint8Array): void {
    const h = decode(frame);
    switch (h.kind) {
      case FrameKind.DocUpdate: {
        const du = decodeDocUpdate(h.payload);
        for (const blob of du.blobs) {
          const v = decodeJson<ProjectionDelta | EditTx>(blob);
          if (isEditTx(v)) continue; // a loopback echo of our own edit; ignore on the UI side
          projectionStore.getState().applyDelta(v);
        }
        this.transport.send(encodeAck(ROOM, du.batchId)); // committed-delta ACK
        break;
      }
      case FrameKind.Ephemeral: {
        const data = decodeJson<Json>(h.payload);
        for (const cb of this.ephemeralListeners) cb(data);
        break;
      }
      case FrameKind.Ack: {
        decodeAck(h.payload); // reconciliation is delta-driven (confirms[]); ack is liveness here
        break;
      }
      default:
        break; // Handshake / Fragment / Ping / Pong handled at the transport/shell layer
    }
  }
}
