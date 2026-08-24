//! Build-time flags this bundle is compiled against.

/** May this bundle construct the in-process dev MockCore?
 *
 *  `true` in `vite dev`, in Vitest, and in the shots harness — all three render the editor with no
 *  engine behind them and are supposed to. `false` in the app's `vite build`, which is the packaged
 *  shell's `frontendDist` and the one bundle that must not contain a core it can answer with; Vite
 *  substitutes the literal, so Rollup drops the mock entirely (76,736 bytes) rather than shipping a
 *  fake core one missing global away from being used. See `transport/session.ts` and ADR-130. */
declare const __MTK_MOCK_CORE__: boolean;
