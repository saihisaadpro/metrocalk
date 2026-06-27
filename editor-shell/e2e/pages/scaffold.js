// The "scaffold" page-object — the swappable selector + action layer for the SHIPPING vanilla-JS UI
// (`editor-shell/web/index.html`). Deliverable 9 (React-migration durability): every acceptance spec
// talks to the build through THIS object's behaviour verbs, never raw selectors, so when M10.1 (prompt 41)
// swaps the UI for the React `/editor`, re-greening the gate = author a sibling page-object with the same
// method surface and point `page()` at it — the specs and the acceptance dimensions are unchanged.
//
// The selectors are derived from the LIVE build's DOM (`web/index.html`), so the inventory can't silently
// drift; `inventory()` returns the full control list the coverage matrix reconciles against the commands.

import { browser, $, $$ } from "@wdio/globals";

const css = (s) => $(s);
const all = (s) => $$(s);

// Invoke a Tauri command from inside the WebView (the same `window.__TAURI__.core.invoke` the UI uses).
// Used to read instrumentation the transparent viewport can't show (IPC counter, gizmo/sim/physics
// state) and to assert command-result behaviour UI-agnostically.
export const invoke = (cmd, args = {}) =>
  browser.execute(async (c, a) => window.__TAURI__.core.invoke(c, a), cmd, args);

const visible = async (sel) => {
  const el = await css(sel);
  if (!(await el.isExisting())) return false;
  return (await el.getCSSProperty("display")).value !== "none";
};

export const scaffold = {
  name: "scaffold (vanilla-JS shell)",

  // ── status / connection / wallet ──────────────────────────────────────────────────────────────
  status: () => css("#status").then((e) => e.getText()),
  reject: () => css("#reject").then((e) => e.getText()),
  count: () => css("#count").then((e) => e.getText()),
  walletBalance: async () => Number((await css("#walletBal").then((e) => e.getText())).trim()),

  async waitConnected(timeout = 60000) {
    await browser.waitUntil(async () => /\d+ entities/.test(await this.count()), {
      timeout,
      timeoutMsg: "editor never showed an entity count (no /core connection?)",
    });
    return this.count();
  },
  async waitStatus(substr, timeout = 10000) {
    await browser.waitUntil(async () => (await this.status()).includes(substr), {
      timeout,
      timeoutMsg: `status never contained "${substr}" (last: ${await this.status()})`,
    });
    return this.status();
  },

  // ── requirers / reveal / bind (north-star #1) ─────────────────────────────────────────────────
  requirers: () => all("#requirers .cand"),
  revealCandidates: () => all("#reveal .cand"),
  boundRows: () => all("#reveal .boundrow"),
  revealText: () => css("#reveal").then((e) => e.getText()),
  async selectRequirer(i = 0) {
    const r = await this.requirers();
    await r[i].click();
  },
  async bindCandidate(i = 0) {
    const c = await this.revealCandidates();
    await c[i].click();
  },

  // ── describe-to-create / generate (north-star #2 / M6) ────────────────────────────────────────
  async describe(query) {
    await (await css("#describe")).setValue(query);
    await (await css("#describeBtn")).click();
  },
  generateButton: () => css("#genBtn"),
  generateVisible: () => visible("#genBtn"),
  async clickGenerate() {
    await (await css("#genBtn")).click();
  },

  // ── inspector field edit ──────────────────────────────────────────────────────────────────────
  inspectorText: () => css("#inspector").then((e) => e.getText()),
  async editFirstField(value) {
    const input = await css("#inspector input");
    if (!(await input.isExisting())) return false;
    await input.setValue(String(value));
    await browser.keys(["Enter"]);
    // Blur the field after committing so a follow-on Ctrl-Z undoes the SCENE, not the input's text (the
    // React editor deliberately bails Ctrl-Z while a text field is focused — don't hijack text undo).
    await browser.execute(() => {
      const el = document.activeElement;
      if (el && typeof el.blur === "function") el.blur();
    });
    return true;
  },

  // ── viewport interactions (pick / orbit / zoom / hover / context menu) ─────────────────────────
  viewport: () => css("#viewport"),
  async pickCenter() {
    await (await this.viewport()).click();
  },
  async orbit(dx = 80, dy = 40) {
    const vp = await this.viewport();
    await browser
      .action("pointer", { parameters: { pointerType: "mouse" } })
      .move({ origin: vp })
      .down({ button: 2 })
      .move({ origin: vp, x: Math.round(dx / 2), y: Math.round(dy / 2) })
      .move({ origin: vp, x: dx, y: dy })
      .up({ button: 2 })
      .perform();
  },
  async zoom(deltaY = -240) {
    // The wheel handler invokes zoom directly; drive it through the command for determinism.
    await invoke("zoom", { delta: deltaY * 0.04 });
  },
  async hoverCenter() {
    const vp = await this.viewport();
    await browser
      .action("pointer", { parameters: { pointerType: "mouse" } })
      .move({ origin: vp, x: 5, y: 5 })
      .move({ origin: vp })
      .perform();
  },
  tooltipVisible: () => visible("#tooltip"),
  tooltipText: () => css("#tooltip").then((e) => e.getText()),

  // ── right-click context menu ──────────────────────────────────────────────────────────────────
  async openContextMenu() {
    const vp = await this.viewport();
    await browser
      .action("pointer", { parameters: { pointerType: "mouse" } })
      .move({ origin: vp })
      .down({ button: 2 })
      .up({ button: 2 })
      .perform();
  },
  contextVisible: () => visible("#ctxmenu"),
  contextItems: () => all("#ctxmenu .ctxitem"),
  contextAction: (action) => css(`#ctxmenu .ctxitem[data-action="${action}"]`),
  async clickContext(action) {
    await (await this.contextAction(action)).click();
  },

  // ── add palette (browse catalog → search → add → generate fall-through) ────────────────────────
  async openPalette() {
    await (await css("#addBtn")).click();
  },
  paletteVisible: () => visible("#palette"),
  async searchPalette(q) {
    await (await css("#palSearch")).setValue(q);
  },
  paletteItems: () => all("#palBody .cand, #palBody .row, #palBody [data-id]"),
  paletteGenerateOffer: () => css("#palGen"),
  async closePalette() {
    await (await css("#palClose")).click();
  },

  // ── wallet / AI-edit ──────────────────────────────────────────────────────────────────────────
  async topUp() {
    await (await css("#topup")).click();
  },
  rustierButton: () => css("#rustier"),
  async clickRustier() {
    await (await css("#rustier")).click();
  },

  // ── undo ──────────────────────────────────────────────────────────────────────────────────────
  async undoButton() {
    await (await css("#undo")).click();
  },
  async undoKey() {
    await browser.keys(["Control", "z"]);
  },

  // ── focus mode ────────────────────────────────────────────────────────────────────────────────
  focusBannerVisible: () => visible("#focusbanner"),
  focusBanner: () => css("#focusbanner"),

  // ── add-palette pick / generate stream-in ─────────────────────────────────────────────────────
  async pickPaletteItem(i = 0) {
    const items = await this.paletteItems();
    await items[i].click();
  },
  async clickPaletteGenerate() {
    await (await this.paletteGenerateOffer()).click();
  },

  // ── M8 physics controls (now in the shipping DOM) ─────────────────────────────────────────────
  async dropBall() {
    await (await css("#dropBall")).click();
  },
  async toggleSim() {
    await (await css("#simToggle")).click();
  },
  async toggleDebugger() {
    await (await css("#dbgToggle")).click();
  },
  async shove() {
    await (await css("#shove")).click();
  },
  async nudgeFriction() {
    await (await css("#nudgeFriction")).click();
  },
  scrubInput: () => css("#scrub"),
  frameLabel: () => css("#frameLbl").then((e) => e.getText()),
  async openImport() {
    await (await css("#importRobot")).click();
  },
  async pasteSampleArm() {
    await (await css("#impSample")).click();
  },
  async importText(text) {
    await (await css("#impText")).setValue(text);
  },
  async runImport() {
    await (await css("#impGo")).click();
  },
  importResult: () => css("#impResult").then((e) => e.getText()),
  async closeImport() {
    await (await css("#impClose")).click();
  },
  // physics instrumentation the transparent viewport can't show (the app itself never polls these).
  physDebug: () => invoke("physics_debug"), // [count, lowestY, contacts]
  simTimeline: () => invoke("sim_timeline"), // [frame, max, running, overlays, bodies]
  physicsCheck: (id) => invoke("physics_check", { id }),

  // ── M9 transform / gizmo / part / solver controls (inspector buttons appear on selection) ──────
  async gizmoMode(mode) {
    const k = mode === "translate" ? "w" : mode === "rotate" ? "e" : "r";
    await browser.keys([k]); // the W/E/R DOM keybindings the user presses
  },
  gizmoDebug: () => invoke("gizmo_debug"), // [mode, hasSel, dragging, space, pivot]
  readTransform: (id) => invoke("read_transform", { id }),
  saveCharButton: () => css("#saveChar"),
  async saveChar() {
    await (await css("#saveChar")).click();
  },
  async dropInstance() {
    await (await css("#dropInst")).click();
  },
  async deactivatePart() {
    await (await css("#deactPart")).click();
  },
  async reparentTo(idOrEmpty) {
    await (await css("#reparentTo")).setValue(idOrEmpty);
    await (await css("#reparentBtn")).click();
  },
  async toggleSnap() {
    await (await css("#snapToggle")).click();
  },
  async snapToNearest() {
    await (await css("#snapNearest")).click();
  },
  async placeBySentence(text) {
    await (await css("#placeSentence")).setValue(text);
    await (await css("#placeBtn")).click();
  },

  // ── the live control inventory (derived from the shipping DOM) ────────────────────────────────
  // Each entry: the control, the command(s) it drives, the workflow that exercises it. The coverage
  // matrix reconciles `command` coverage against this list so a new button can't slip in unexercised.
  inventory() {
    return [
      // M1–M7 user-facing surface
      { control: "#topup", command: ["top_up", "wallet_info"], workflow: "wallet/top-up" },
      { control: "#undo", command: ["undo"], workflow: "undo (button + Ctrl-Z)" },
      { control: "#addBtn", command: ["catalog"], workflow: "add-palette/open" },
      { control: "#palSearch", command: ["catalog_search"], workflow: "add-palette/search" },
      { control: "#palBody item", command: ["add_item"], workflow: "add-palette/pick" },
      { control: "#palGen", command: ["generate"], workflow: "add-palette/generate-fallthrough" },
      { control: "#palClose", command: [], workflow: "add-palette/close (esc)" },
      { control: "#describe + #describeBtn", command: ["describe"], workflow: "describe-to-create" },
      { control: "#genBtn", command: ["generate"], workflow: "generate (opt-in)" },
      { control: "#requirers .cand", command: ["reveal_targets"], workflow: "select requirer → reveal" },
      { control: "#reveal .cand", command: ["bind_target"], workflow: "bind-by-intent" },
      { control: "#inspector input", command: ["submit_edit"], workflow: "field edit" },
      { control: "#viewport (left-click)", command: ["viewport_pick"], workflow: "viewport pick" },
      { control: "#viewport (right-drag)", command: ["drag_start", "drag_end"], workflow: "orbit" },
      { control: "#viewport (wheel)", command: ["zoom"], workflow: "zoom" },
      { control: "#viewport (hover)", command: ["viewport_peek", "entity_details"], workflow: "hover peek" },
      { control: "#ctxmenu (right-click)", command: ["entity_actions"], workflow: "context reveal" },
      { control: '#ctxmenu [data-action=remove]', command: ["remove_entity"], workflow: "context/remove" },
      { control: '#ctxmenu [data-action=duplicate]', command: ["duplicate_entity"], workflow: "context/duplicate" },
      { control: '#ctxmenu [data-action=focus]', command: ["focus_entity", "unfocus", "focus_debug"], workflow: "focus mode" },
      { control: '#ctxmenu [data-action=inspect]', command: ["entity_details"], workflow: "context/inspect" },
      { control: "#rustier", command: ["ai_edit"], workflow: "AI-edit (make it rustier)" },
      { control: "connect (boot)", command: ["connect"], workflow: "launch → composite → connect" },
      // M8 physics surface (now in the build — derived live)
      { control: "#dropBall", command: ["spawn_body"], workflow: "physics/drop-ball" },
      { control: "#simToggle", command: ["set_sim_running"], workflow: "physics/pause-resume" },
      { control: "#dbgToggle", command: ["sim_overlay"], workflow: "physics/debugger overlay" },
      { control: "#shove", command: ["sim_shove"], workflow: "physics/shove" },
      { control: "#nudgeFriction", command: ["submit_edit"], workflow: "physics/edit-at-pause friction" },
      { control: "#scrub", command: ["sim_scrub", "sim_timeline"], workflow: "physics/scrub timeline" },
      { control: "#importRobot + #impGo", command: ["import_interchange"], workflow: "interchange/URDF-USD import" },
      { control: "context/make-dynamic", command: ["make_dynamic", "physics_check", "physics_fix"], workflow: "physics/make-dynamic + fix" },
      // M9 transform surface (now in the build — derived live)
      { control: "#viewport (gizmo drag) + W/E/R", command: ["gizmo_pick_drag", "gizmo_drag_end", "gizmo_mode", "gizmo_select", "gizmo_space_toggle", "gizmo_pivot_toggle"], workflow: "gizmo transform" },
      { control: "#saveChar", command: ["save_character"], workflow: "G2/save character" },
      { control: "#dropInst", command: ["instantiate_character"], workflow: "G2/drop instance" },
      { control: "#deactPart", command: ["set_part_active"], workflow: "G2/deactivate-not-delete" },
      { control: "#reparentBtn", command: ["reparent_part"], workflow: "G2/reparent" },
      { control: "#snapToggle", command: ["set_snap"], workflow: "G4/magnetic snap toggle" },
      { control: "#snapNearest", command: ["snap_query", "apply_constraint"], workflow: "G4/snap-to-nearest" },
      { control: "#placeBtn", command: ["placement_sentence"], workflow: "G4/place-by-sentence" },
      // M11.3 lighting surface (ADR-042) — authored light entities + the lighting_debug read the gate keys off
      { control: "lighting (add_light + lighting_debug)", command: ["add_light", "lighting_debug"], workflow: "lighting/author + shadow-caster" },
    ];
  },
};

// The React `/editor` DELTAS over the scaffold verbs (M10.1 swap): the React UI keeps the scaffold's stable
// ids, so almost every selector is identical — only the verbs whose *interaction shape* genuinely differs
// are overridden here (undo is Ctrl-Z, not a `#undo` button; AI-edit is a two-step confirm). The specs +
// acceptance dimensions never change — that's the swappable-layer point (prompt-40 d9 / M10.1 d7).
const reactDeltas = {
  name: "react (/editor)",
  // The React catalog is the always-visible AssetBrowser — a PERSISTENT panel, not a closeable modal like
  // the scaffold's `#palette`. Specs that assert "the palette closes on esc" guard on this flag (a
  // behavioral difference no page-object selector swap can paper over — the panel is meant to stay open).
  persistentCatalog: true,
  // React undo = a global Ctrl-Z keydown (no `#undo` button in the React chrome).
  async undoButton() {
    await browser.keys(["Control", "z"]);
  },
  // AI-edit ("rustier") is now a deliberate two-step spend: open the confirm, then Apply (M10.10 / C3).
  async clickRustier() {
    await (await css("#rustier")).click();
    const apply = await css("#rustierApply");
    if (await apply.isExisting()) await apply.click();
  },

  // ── add-palette → the React AssetBrowser (M10.2): the React UI has no separate `#palette` modal — the
  // browse/search/place/generate-fallthrough catalog surface IS the always-visible AssetBrowser
  // (`#assetbrowser` · `#assetSearch` · `[data-testid=asset-item]` · `[data-testid=asset-seam]`). Map the
  // scaffold palette verbs onto it so the add-palette spec re-greens by page-object swap, not a rewrite. ──
  async openPalette() {
    // always visible — focus the search so a follow-on type lands there (no modal to open)
    const s = await css("#assetSearch");
    if (await s.isExisting()) await s.click();
  },
  paletteVisible: () => visible("#assetbrowser"),
  paletteItems: () => all('[data-testid="asset-item"]'),
  async searchPalette(q) {
    await (await css("#assetSearch")).setValue(q);
  },
  paletteGenerateOffer: () => css('[data-testid="asset-seam"]'),
  async closePalette() {
    /* nothing to close — the browser is a persistent panel */
  },
  async pickPaletteItem(i = 0) {
    const items = await all('[data-testid="asset-item"]');
    await items[i].click();
  },

  // ── M10.3 File menu (React-only chrome) — the first-session journey (M10.5). `New` is a pure command
  // (empty scene, no dialog); `Save`/`Open` open the OWED native dialogs, so the journey drives persistence
  // via `save_project`/`open_project({path})` invokes (the command layer the dialogs would call). ──
  async newProject() {
    await (await css("#fileMenu")).click(); // open the File menu
    await (await css("#fileNew")).click(); // New project → new_project (a fresh empty scene)
    const discard = await css("#guardDiscard"); // if a dirty-guard appeared, discard + proceed
    if (await discard.isExisting()) await discard.click();
  },
  projectDirty: () => visible("#projectDirty"),

  // ── M10.4 Play / Stop (React PlayControls) — the test-it leg of the journey ──
  async play() {
    await (await css('[data-testid="play"]')).click();
  },
  async stopPlay() {
    await (await css('[data-testid="stop"]')).click();
  },
  playing: () => visible('[data-testid="playIndicator"]'),
  playIndicatorText: () => css('[data-testid="playIndicator"]').then((e) => e.getText()),
};

export function page() {
  // The React `/editor` is the PRODUCTION UI (`frontendDist`→React, M10.1; the vanilla scaffold
  // `web/index.html` is retired). The `scaffold` object above is now the BASE selector/verb layer — the
  // stable ids the React UI keeps — that the React deltas extend. There is no separate scaffold UI to
  // target anymore, so the page-object is unconditionally React. (`MTK_UI` is kept honored as a legacy
  // no-op for any external runner that still sets it.)
  return { ...scaffold, ...reactDeltas };
}
