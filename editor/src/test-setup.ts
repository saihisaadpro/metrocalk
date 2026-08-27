import { afterEach } from "vitest";
import { cleanup, configure } from "@testing-library/react";

// HOW LONG A WAIT MAY TAKE, STATED ONCE.
//
// Testing Library's `asyncUtilTimeout` is 1000ms — the bound on every `findBy*` and every bare
// `waitFor` in the repository. That is a WALL-CLOCK number, and vitest runs these 76 files in parallel
// workers, so on a machine that is also compiling something it is a bound on the machine rather than
// on the behaviour under test. Measured: `ViewportToolbar`'s space toggle and `App`'s first-run scene
// both pass in isolation and both miss 1000ms inside the full parallel run — a verdict decided by what
// else is running, which is the isolation rule in `<test_and_ci_discipline>` 4 read the other way
// round.
//
// Raising it does NOT make a failing test pass: `waitFor` returns the moment its callback succeeds, so
// this only changes how long a test that is genuinely never going to succeed spends failing, and the
// per-test timeout still bounds that. What it buys is that the same tree gives the same answer on a
// busy machine and a quiet one. Stated here rather than at each call site for the reason the ADR-156
// commit gives about `App.test.tsx`'s inline `10_000`: a number written twice is the one that goes
// stale, and that one already had.
configure({ asyncUtilTimeout: 15_000 });

// Unmount React trees between tests so render counters don't leak across cases.
afterEach(() => cleanup());

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
