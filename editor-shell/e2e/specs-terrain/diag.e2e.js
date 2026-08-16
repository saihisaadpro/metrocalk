// Diagnostic: what does the runtime actually report after a terrain is created?
import { execFileSync } from "node:child_process";
import { mkdirSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { invoke } from "../pages/scaffold.js";

const dir = path.dirname(fileURLToPath(import.meta.url));
const shots = path.resolve(dir, "../.shots-terrain");
const capture = path.resolve(dir, "../scripts/capture-composited-window.ps1");
mkdirSync(shots, { recursive: true });

describe("terrain diagnostic", () => {
  it("reports what the streamer is doing", async () => {
    await invoke("new_project");
    const reply = await invoke("terrain_create", { preset: "rolling-hills" });
    console.log(`[diag] create ok=${reply.ok} msg=${reply.message}`);
    for (let i = 0; i < 12; i += 1) {
      await browser.pause(2000);
      const s = await invoke("terrain_stats");
      const cam = await invoke("camera_debug");
      console.log(
        `[diag] t=${(i + 1) * 2}s resident=${s.residentChunks} visible=${s.visibleChunks} ` +
          `pending=${s.pendingBuilds} done=${s.completedBuilds} tris=${s.drawnTriangles} ` +
          `mesh=${s.meshMb.toFixed(1)}MB tex=${s.textureMb.toFixed(1)}MB over=${s.overBudget} ` +
          `culledF=${s.culledFrustum} culledH=${s.culledHorizon} cam=${JSON.stringify(cam)}`,
      );
    }
    const h = await invoke("terrain_height", { x: 1024, z: 1024 });
    console.log(`[diag] terrain height at world centre: ${h}`);
    execFileSync(
      "powershell.exe",
      ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", capture, "-Out", path.join(shots, "diag.png")],
      { stdio: "pipe" },
    );
  });
});
