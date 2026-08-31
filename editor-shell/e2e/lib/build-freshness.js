// Does the .exe under test actually contain the tree under test?
//
// WHY THIS EXISTS. On 2026-08-26 a look-dev run failed with `Command set_presentation_set not found`
// against a binary that had been rebuilt minutes earlier. The command was present in `main.rs`, was
// registered in `tauri::generate_handler!`, and rustc raised no dead-code warning for it — every static
// check said it was there. It was not in the binary, because `main.rs` had been written 09:34:58 and the
// binary linked 09:29:58. Five minutes of skew, and the harness had no way to see it: a Tauri command
// that does not exist is reported at the moment it is INVOKED, which in a film run is twenty minutes in.
//
// The failure mode is worse than a wasted run. A stale binary does not usually 404 - it usually just
// behaves like the older build, silently, and every measurement taken from it is attributed to the
// source in the working tree. That is how a fix gets credited to a film that never contained it.
//
// So this is the same guard the frontend bundle already has, one layer down: compare the artifact to the
// tree that claims to have produced it, and refuse to roll if the tree is newer.

import { existsSync, readdirSync, statSync } from "node:fs";
import path from "node:path";

/**
 * The newest mtime under `dir` among files matching `test`, with the file that carried it.
 *
 * Directory walk rather than a glob dependency: this runs before the harness has done anything, and a
 * check that cannot run without installing something is a check that gets skipped.
 */
function newestUnder(dir, test, worst = { ms: 0, file: null }) {
  if (!existsSync(dir)) return worst;
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      if (entry.name === "node_modules" || entry.name === "target" || entry.name === ".git") continue;
      worst = newestUnder(full, test, worst);
    } else if (test(entry.name)) {
      const ms = statSync(full).mtimeMs;
      if (ms > worst.ms) worst = { ms, file: full };
    }
  }
  return worst;
}

const isRust = (name) => name.endsWith(".rs");
const isBundled = (name) => name.endsWith(".js") || name.endsWith(".css") || name.endsWith(".html");

/**
 * Throw unless `exe` is newer than every source that is compiled or embedded into it.
 *
 * `repoRoot` is the directory holding `editor-shell/`, `animation/` and `editor/`.
 *
 * Sources checked, and why each one: the Tauri app crate (`editor-shell/src-tauri/src`) and the shell
 * library it links (`editor-shell/src`) are the binary's own code; `animation/src` carries the shot
 * solver, which is the thing a cinematic run is usually measuring; `editor/dist` is embedded at COMPILE
 * time by `frontendDist`, so a rebuilt frontend that has not been re-linked is invisible to the app.
 *
 * Deliberately NOT a warning. A warning here is read after the run, next to the numbers it invalidates.
 */
export function assertExeMatchesTree(exe, repoRoot) {
  if (!existsSync(exe)) {
    throw new Error(`The application under test does not exist: ${exe}`);
  }
  const exeMs = statSync(exe).mtimeMs;

  const sources = [
    [path.join(repoRoot, "editor-shell/src-tauri/src"), isRust, "the Tauri app crate"],
    [path.join(repoRoot, "editor-shell/src"), isRust, "the editor-shell library"],
    [path.join(repoRoot, "animation/src"), isRust, "the animation crate (shot solver)"],
    [path.join(repoRoot, "editor/dist"), isBundled, "the embedded frontend bundle"],
  ];

  const stale = [];
  for (const [dir, test, label] of sources) {
    const newest = newestUnder(dir, test);
    if (newest.file && newest.ms > exeMs) {
      stale.push(
        `  ${label}: ${path.relative(repoRoot, newest.file)} ` +
        `(${new Date(newest.ms).toISOString()}) is newer than the binary`,
      );
    }
  }

  if (stale.length) {
    throw new Error(
      `The application under test predates the source tree, so this run would measure code that is not ` +
      `in the working copy.\n` +
      `  binary: ${exe} (${new Date(exeMs).toISOString()})\n` +
      `${stale.join("\n")}\n` +
      `Rebuild before rolling:\n` +
      `  cargo build --release --manifest-path editor-shell/src-tauri/Cargo.toml\n` +
      `(and 'vite build' first if the frontend changed - editor/dist is embedded at compile time).`,
    );
  }

  return { exe, exeMs };
}
