import { afterEach } from "vitest";
import { cleanup, configure } from "@testing-library/react";

// Unmount React trees between tests so render counters don't leak across cases.
afterEach(() => cleanup());

// **A DYNAMIC IMPORT IS NOT A SPEED GATE.** `App` code-splits its heaviest panels — the command
// palette, Pipe Forge, the context menu, every workspace — behind `React.lazy`, so a `findBy*` that
// waits for one of them is waiting for Vite to transform and evaluate a module, not for a render.
// Testing Library's default async timeout is 1000 ms, which turns that wait into an assertion about
// how busy the machine is: four `App` tests fail together whenever a sibling build holds the cores,
// and pass alone, on a tree with nothing wrong in it (`<test_and_ci_discipline>` 4 — a flake is a
// failure, and the fix is the isolation, not a retry).
//
// Ten seconds is not "more patience": it is far beyond any plausible module evaluation and far below
// Vitest's own 5 s-per-hook / test budget, so a wait that genuinely never resolves still fails the
// test rather than hanging. Nothing here asserts a duration; the tests that measure interaction cost
// measure it directly.
configure({ asyncUtilTimeout: 10_000 });

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
