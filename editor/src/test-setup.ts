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
// `App.test.tsx` flaked on THREE DIFFERENT assertions across four full-suite runs (the C7 wallet
// chain at the 1000 ms default, and the inspector-on-select wait at a locally-written 10 s), and
// passed in isolation every time, taking 11.4 s for the file alone against 21.2 s in a full run.
// That is the signature `<test_and_ci_discipline>` 4 names: a flake is a failure and the fix is the
// isolation, not a retry — and 330781a already deleted one wall-clock assertion here for it.
//
// The distinction that makes widening this the RIGHT repair rather than the lazy one: 330781a
// removed a bound that WAS the assertion (`expect(dt).toBeLessThan(100)` — a claim about speed).
// This bound asserts nothing. It is only how long a test is willing to wait before declaring that
// something will never happen, and a test that gives up on an import in one second is measuring the
// machine, not the editor. Anything genuinely broken still fails, one wait later.
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
