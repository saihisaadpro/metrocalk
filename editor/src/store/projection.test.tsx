import { startTransition } from "react";
import { act, render } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";
import type { EntityProjection } from "../transport/protocol";
import {
  projectionStore,
  useDisplayedEntity,
  useFieldValue,
  useSummary,
} from "./projection";

const N = 5000;

function seed(n: number): EntityProjection[] {
  const out: EntityProjection[] = [];
  for (let i = 0; i < n; i++) {
    out.push({
      id: `e${i}`,
      name: `Entity ${i}`,
      parentId: i === 0 ? null : "e0",
      components: { Transform: { x: 0, y: 0, z: 0 } },
    });
  }
  return out;
}

const counts: Record<string, number> = {};
const bump = (k: string) => (counts[k] = (counts[k] ?? 0) + 1);

// A list row subscribes ONLY to the {id,name,parentId} summary (the tree never re-renders on a field edit).
function Row({ id }: { id: string }) {
  const s = useSummary(id);
  bump(`row:${id}`);
  return <div data-testid={`row-${id}`}>{s?.name}</div>;
}

// The inspector subscribes to the full displayed (base ⊕ pending) projection of the selected entity.
function Inspector({ id }: { id: string }) {
  const e = useDisplayedEntity(id);
  bump(`insp:${id}`);
  return <div data-testid="insp">{JSON.stringify(e?.components ?? {})}</div>;
}

beforeEach(() => {
  for (const k of Object.keys(counts)) delete counts[k];
  projectionStore.getState().reset();
});

describe("projection store — selective subscription at 5k entities", () => {
  it("a field-edit delta re-renders only the edited entity's detail, not the 5k tree", () => {
    projectionStore.getState().bulkLoad(seed(N));
    render(
      <>
        {Array.from({ length: N }, (_, i) => (
          <Row key={i} id={`e${i}`} />
        ))}
        <Inspector id="e42" />
      </>,
    );

    // initial: every row + inspector rendered exactly once
    expect(counts["row:e42"]).toBe(1);
    expect(counts["insp:e42"]).toBe(1);
    const rowRenders = () => Object.keys(counts).filter((k) => k.startsWith("row:")).length;
    expect(rowRenders()).toBe(N);

    const before = { ...counts };
    const t0 = performance.now();
    act(() => {
      projectionStore.getState().applyDelta({
        ops: [{ op: "setField", id: "e42", component: "Transform", field: "x", value: 9 }],
      });
    });
    const dt = performance.now() - t0;

    // exactly the inspector for e42 re-rendered; NO row re-rendered (summary unchanged)
    expect(counts["insp:e42"]).toBe(2);
    const changedRows = Object.keys(counts).filter((k) => k.startsWith("row:") && counts[k] !== before[k]);
    expect(changedRows).toEqual([]);
    // eslint-disable-next-line no-console
    console.log(`[5k] single-entity field-edit apply+render: ${dt.toFixed(3)} ms (n=${N})`);
    expect(dt).toBeLessThan(100); // generous bound; the real number is logged above

    // a NAME change touches the summary → exactly that one row re-renders, inspector untouched
    const before2 = { ...counts };
    act(() => {
      projectionStore.getState().applyDelta({ ops: [{ op: "upsert", id: "e7", name: "Renamed 7" }] });
    });
    expect(counts["row:e7"]).toBe((before2["row:e7"] ?? 0) + 1);
    expect(counts["insp:e42"]).toBe(before2["insp:e42"]); // detail did not re-render on a tree change
    const changed2 = Object.keys(counts).filter((k) => k.startsWith("row:") && counts[k] !== before2[k]);
    expect(changed2).toEqual(["row:e7"]);
  });

  it("displayed[id] stays reference-identical to base[id] when no pending op touches it", () => {
    projectionStore.getState().bulkLoad(seed(100));
    const s = projectionStore.getState();
    expect(s.displayed["e5"]).toBe(s.base["e5"]); // same ref ⇒ subscribers don't re-render
    s.optimisticEdit({ clientOpId: "op1", intent: { kind: "setField", id: "e5", component: "Transform", field: "x", value: 1 } });
    const s2 = projectionStore.getState();
    expect(s2.displayed["e5"]).not.toBe(s2.base["e5"]); // overlaid ⇒ new ref
    expect(s2.displayed["e6"]).toBe(s2.base["e6"]); // untouched neighbour ⇒ still identical
  });
});

describe("tear-free under a concurrent transition", () => {
  it("a multi-field atomic delta is never observed half-applied across renders", () => {
    projectionStore.getState().bulkLoad(seed(50));
    const observed: Array<[unknown, unknown]> = [];
    function Pair() {
      const x = useFieldValue("e1", "Transform", "x");
      const y = useFieldValue("e1", "Transform", "y");
      observed.push([x, y]);
      return <div data-testid="pair">{`${x},${y}`}</div>;
    }
    render(<Pair />);
    act(() => {
      startTransition(() => {
        // both fields move together in ONE atomic store update (single set())
        projectionStore.getState().applyDelta({
          ops: [
            { op: "setField", id: "e1", component: "Transform", field: "x", value: 5 },
            { op: "setField", id: "e1", component: "Transform", field: "y", value: 5 },
          ],
        });
      });
    });
    // every observed render shows a consistent (x===y) snapshot — never (5,0) or (0,5)
    for (const [x, y] of observed) expect(x).toBe(y);
    expect(observed.at(-1)).toEqual([5, 5]);
  });
});
