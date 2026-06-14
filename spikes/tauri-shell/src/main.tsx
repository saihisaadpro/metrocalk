// Entry: selftest the JS↔Rust roundtrip, then dispatch on the mode the Rust side reports.
import { invoke } from "@tauri-apps/api/core";

interface SelfTest {
  ok: boolean;
  webview2: string;
  mode: string;
  run: string;
  seconds: number;
}

async function main() {
  const out = document.getElementById("out")!;
  let info: SelfTest;
  try {
    info = await invoke<SelfTest>("selftest");
  } catch (e) {
    out.textContent = "selftest ERROR → " + String(e);
    return;
  }
  out.textContent = `mode=${info.mode} · webview2=${info.webview2} · run=${info.run}`;

  if (info.mode === "bench") {
    const { runBench1a } = await import("./bench");
    await runBench1a(info.run, info.seconds);
  } else if (info.mode === "composite") {
    const { mountComposite } = await import("./composite");
    mountComposite();
  } else {
    out.textContent = "selftest OK → " + JSON.stringify(info, null, 2);
  }
}

main();
