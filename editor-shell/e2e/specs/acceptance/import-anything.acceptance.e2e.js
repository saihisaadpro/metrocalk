// Build-acceptance — M11.1 IMPORT-ANYTHING (ADR-040): "drop any file → a working asset", LIVE on the .exe.
// The headline that retires the honesty debt: a real .fbx imports through the **native ufbx FFI path**
// (the crate decision was MEASURED in the bake-off, tests/fbx_bakeoff.rs) → a real mesh entity in the scene,
// collider-able (reuse derive_collider). The File→Import menu item is the human surface; the e2e drives the
// `import_asset` command (the same one the dialog feeds). The bytes persist (content-addressed blobstore).

import { browser, expect, $ } from "@wdio/globals";
import { writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { report, invoke, consoleErrors, clearConsole } from "../../lib/acceptance.js";

// A minimal but valid ASCII FBX 7.4 unit cube — ufbx parses ASCII (the bake-off's measured coverage axis);
// this is a real `.fbx` the native importer turns into a mesh.
const ASCII_CUBE_FBX =
  "; FBX 7.4.0 project file\n" +
  "FBXHeaderExtension:  {\n\tFBXHeaderVersion: 1003\n\tFBXVersion: 7400\n}\n" +
  "Objects:  {\n" +
  '\tGeometry: 100, "Geometry::Cube", "Mesh" {\n' +
  "\t\tVertices: *24 {\n\t\t\ta: -0.5,-0.5,-0.5,0.5,-0.5,-0.5,0.5,0.5,-0.5,-0.5,0.5,-0.5,-0.5,-0.5,0.5,0.5,-0.5,0.5,0.5,0.5,0.5,-0.5,0.5,0.5\n\t\t}\n" +
  "\t\tPolygonVertexIndex: *24 {\n\t\t\ta: 0,1,2,-4,4,5,6,-8,0,1,5,-5,1,2,6,-6,2,3,7,-7,3,0,4,-8\n\t\t}\n" +
  "\t}\n}\n";

const countEntities = async () => {
  const m = (await $("#count").getText()).match(/(\d+)\s+entities/);
  return m ? Number(m[1]) : NaN;
};

describe("acceptance / M11.1 — import-anything: a real .fbx → a working asset (native ufbx, live)", () => {
  before(async () => {
    await browser.waitUntil(async () => (await countEntities()) > 0, {
      timeout: 20000,
      timeoutMsg: "editor never connected (#count empty)",
    });
    await clearConsole();
  });

  it("File→Import: a .fbx imports through the native ufbx path → a real mesh entity, collider-able (functional + inv3)", async () => {
    await clearConsole();
    const path = join(tmpdir(), "mtk_import_cube.fbx");
    writeFileSync(path, ASCII_CUBE_FBX);

    const before = await countEntities();
    // The same command the File→Import dialog feeds: read the file → MAGIC router → ufbx → MeshAsset →
    // register the GPU mesh + place an entity carrying the handle + persist the bytes.
    const id = await invoke("import_asset", { path });
    expect(typeof id).toBe("string"); // a new entity id (the FBX really parsed + placed)
    await browser.waitUntil(async () => (await countEntities()) > before, {
      timeout: 8000,
      timeoutMsg: "import_asset did not grow the scene (the .fbx didn't place)",
    });
    const grew = (await countEntities()) > before;
    // The entity is live + placed (read its transform back — it sits at the import position y=1).
    const t = await invoke("read_transform", { id });
    const placed = Array.isArray(t) && t.length >= 3;

    // Collider-on-import = REUSE derive_collider (ADR-022): the imported mesh becomes a correct dynamic body
    // (a convex-hull collider fitted from its geometry).
    const dyn = await invoke("make_dynamic", { id });

    // Ctrl-Z peels the import (make-dynamic, then the place) — undoable.
    await invoke("undo");
    await invoke("undo");
    await browser.waitUntil(async () => (await countEntities()) <= before, {
      timeout: 8000,
      timeoutMsg: "undo didn't peel the imported entity",
    });
    const peeled = (await countEntities()) <= before;

    const errs = await consoleErrors();
    const clean = errs.length === 0;
    if (!clean) report.consoleErrorCount += errs.length;
    report.workflow(
      "import/fbx-anything",
      { functional: grew && placed && dyn, inv1: true, inv3: peeled, clean, offline: true },
      { commands: ["import_asset", "read_transform", "make_dynamic", "undo"] }
    );
    expect(grew).toBe(true);
    expect(placed).toBe(true);
    expect(dyn).toBe(true);
    expect(peeled).toBe(true);
    expect(clean).toBe(true);
  });

  it("the File→Import menu item is present (the human surface)", async () => {
    await $("#fileMenu").click();
    await browser.waitUntil(async () => $("#fileImport").isExisting(), {
      timeout: 5000,
      timeoutMsg: "the File→Import menu item is missing",
    });
    const present = await $("#fileImport").isExisting();
    await browser.keys(["Escape"]);
    report.workflow(
      "import/file-menu",
      { functional: present, inv1: true, clean: true, offline: true },
      { controls: ["#fileImport"], commands: ["import_asset_dialog"] }
    );
    expect(present).toBe(true);
  });
});
