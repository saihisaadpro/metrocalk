import { afterEach } from "vitest";
import { cleanup, configure } from "@testing-library/react";

// Unmount React trees between tests so render counters don't leak across cases.
afterEach(() => cleanup());

// `findBy*`/`waitFor` WAIT ON A LOADED MACHINE, and testing-library's default budget is 1000ms of
// WALL CLOCK. Vitest runs 77 files across whatever cores are free, and most of what these waits are
// waiting for is a `React.lazy` dynamic import resolving — which is not slow, it is queued.
//
// Measured on this tree, 2026-08-26: three consecutive full-suite runs on an otherwise idle box each
// failed 2 of 523, always in `App.test.tsx`, never the same two, with `Unable to find an element by:
// [data-testid="describe"]` — the Build workspace's lazily-imported `DescribeBar`. Every one of those
// files is 13/13 green run alone. A failure population that moves between runs and empties in
// isolation is contention, and a flake is a defect in the harness, not noise to retry.
//
// This raises the ASYNC-UTIL budget only. The per-test timeout — the thing that catches a real hang —
// stays at its 5000ms default everywhere except the two heaviest files, which carry their own
// measured number. 4000 is under that default on purpose: a wait that outlives its own test reports
// as a timeout with none of the DOM that explains it.
configure({ asyncUtilTimeout: 4000 });

// React Flow measures its container via ResizeObserver, which jsdom lacks — stub it.
if (!("ResizeObserver" in globalThis)) {
  (globalThis as unknown as { ResizeObserver: unknown }).ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
}

// jsdom 25 renders the `inert` attribute but does not reflect it as an `HTMLElement.inert` property the way
// browsers do, so collapsed-region assertions would read `undefined`. Mirror the attribute.
if (typeof HTMLElement !== "undefined" && !("inert" in HTMLElement.prototype)) {
  Object.defineProperty(HTMLElement.prototype, "inert", {
    configurable: true,
    get(this: HTMLElement) {
      return this.hasAttribute("inert");
    },
    set(this: HTMLElement, value: boolean) {
      if (value) this.setAttribute("inert", "");
      else this.removeAttribute("inert");
    },
  });
}
