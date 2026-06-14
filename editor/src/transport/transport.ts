//! The TS `DeltaTransport` — byte-only, mirroring the M2.4 Rust trait (`send` / `setOnRecv` /
//! `connectionState`). Two bindings: an in-process pair (browser, zero-copy synchronous loopback —
//! the Loro doc is local, ADR-006) and a desktop binding the M2.6 shell points at the real Tauri
//! Channel IPC. All protocol logic lives above this (`client.ts`); a binding only moves bytes.

export type ConnectionState = "connecting" | "connected" | "disconnected";

export interface DeltaTransport {
  send(frame: Uint8Array): void;
  setOnRecv(cb: (frame: Uint8Array) => void): void;
  connectionState(): ConnectionState;
}

class InProcess implements DeltaTransport {
  private peerCb: { fn: ((f: Uint8Array) => void) | null };
  private myCb: { fn: ((f: Uint8Array) => void) | null };
  constructor(mine: { fn: ((f: Uint8Array) => void) | null }, peer: { fn: ((f: Uint8Array) => void) | null }) {
    this.myCb = mine;
    this.peerCb = peer;
  }
  send(frame: Uint8Array): void {
    // synchronous loopback to the peer's installed callback (copy so the receiver owns its bytes)
    this.peerCb.fn?.(frame.slice());
  }
  setOnRecv(cb: (f: Uint8Array) => void): void {
    this.myCb.fn = cb;
  }
  connectionState(): ConnectionState {
    return "connected";
  }
}

/** A connected in-process pair (e.g. UI ↔ in-browser WASM core / mock core). */
export function inProcessPair(): [DeltaTransport, DeltaTransport] {
  const a = { fn: null as ((f: Uint8Array) => void) | null };
  const b = { fn: null as ((f: Uint8Array) => void) | null };
  return [new InProcess(a, b), new InProcess(b, a)];
}

/**
 * Desktop binding — **stub wired for real in M2.6**. The shell injects `send` (→ the Tauri
 * `invoke` raw-binary / Channel path) and pushes inbound frames via [`deliver`]. The byte contract
 * is identical to the in-process binding, so swapping bindings is the only M2.6 change here.
 */
export class DesktopTransport implements DeltaTransport {
  private onRecv: ((f: Uint8Array) => void) | null = null;
  private state: ConnectionState = "connecting";
  constructor(private sink: (frame: Uint8Array) => void) {}
  send(frame: Uint8Array): void {
    this.sink(frame);
  }
  setOnRecv(cb: (f: Uint8Array) => void): void {
    this.onRecv = cb;
  }
  /** The shell's inbound IPC handler calls this with each frame from the core. */
  deliver(frame: Uint8Array): void {
    this.onRecv?.(frame);
  }
  markConnected(): void {
    this.state = "connected";
  }
  connectionState(): ConnectionState {
    return this.state;
  }
}
