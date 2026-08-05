import { expect, test } from "vitest";
import { normalizeSurfacePoint } from "./viewportCoordinates";

test("normalizes native picking against the full render surface", () => {
  expect(normalizeSurfacePoint(600, 200, 1200, 800)).toEqual({ x: 0.5, y: 0.25 });
  expect(normalizeSurfacePoint(-10, 900, 1200, 800)).toEqual({ x: 0, y: 1 });
});
