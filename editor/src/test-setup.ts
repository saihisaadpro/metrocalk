import { afterEach } from "vitest";
import { cleanup, configure } from "@testing-library/react";

// Unmount React trees between tests so render counters don't leak across cases.
afterEach(() => cleanup());

// ONE DEADLINE FOR "THE LAZY WORKSPACE HAS RESOLVED", STATED HERE.
//
// Testing Library's default `asyncUtilTimeout` is 1000 ms, and in this repository almost every
// `findBy`/`waitFor` in a shell test is waiting on a DYNAMIC IMPORT: `EditorDocks` code-splits each
// workspace behind one Suspense boundary, so `#inspector` does not exist until every lazy child in
// that boundary has resolved. Resolving a chunk is not a performance claim — it is module loading,
// under a runner that deliberately saturates the machine with 78 files at once.
//
// TWO LANES MEASURED THIS INDEPENDENTLY AND AGREED ABOUT THE CAUSE. One saw three consecutive
// full-suite runs each fail 2 of 523, always in `App.test.tsx`, never the same two, always
// `Unable to find an element by: [data-testid="describe"]` — the Build workspace's lazily-imported
// `DescribeBar`. The other saw three DIFFERENT assertions flake across four runs, the file taking
// 11.4 s alone against 21.2 s under load. Both files are green run alone. A failure population that
// moves between runs and empties in isolation is contention, and a flake is a defect in the harness.
//
// THE TWO NUMBERS ARE ONE DECISION, which is why the smaller of the two proposals is not taken here.
// 4000 ms was chosen to stay under the 5000 ms per-test default, on the sound argument that a wait
// outliving its own test reports as a timeout with none of the DOM that explains it — but the fix for
// that is to move the per-test budget too, and `vite.config.ts` does exactly that (`testTimeout:
// 30_000`). With both moved, 15 s is how long a test is willing to wait before declaring that
// something will never happen, and anything genuinely broken still fails one wait later.
//
// The distinction that makes widening this the RIGHT repair rather than the lazy one: 330781a removed
// a bound that WAS the assertion (`expect(dt).toBeLessThan(100)` — a claim about speed). This bound
// asserts nothing about the editor at all.
configure({ asyncUtilTimeout: 15_000 });

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
