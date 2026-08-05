/**
 * Native picking rays are projected across the complete wgpu/WebView surface, not the cropped DOM stage.
 * Pointer coordinates must therefore be normalized against the full window even when docks change the
 * visible viewport rectangle. Keeping this conversion shared prevents pipe placement and gizmo picking
 * from drifting apart as panels open or collapse.
 */
export function normalizeSurfacePoint(
  clientX: number,
  clientY: number,
  width = window.innerWidth,
  height = window.innerHeight,
): { x: number; y: number } {
  const safeWidth = Math.max(1, width);
  const safeHeight = Math.max(1, height);
  return {
    x: Math.min(1, Math.max(0, clientX / safeWidth)),
    y: Math.min(1, Math.max(0, clientY / safeHeight)),
  };
}
