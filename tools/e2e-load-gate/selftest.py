"""`gate.py --self-test` — every case runs twice: drifted, then repaired.

Asserting only that a drifted tree produces a finding lets a case pass for the wrong reason — some
neighbouring guard fires on the same input, the case goes green, and the guard it was written for
can be deleted with nobody noticing. So each case must also be **absent** on the repaired tree, and
the repaired tree must be entirely clean. That pair is the mutation test, run on every CI run rather
than the one time somebody remembers to revert a fix by hand.

Case 1 is not a hypothetical. It is prompt 40's incident, reduced: `lib/acceptance.js` re-exports
`invoke`, every acceptance spec imports it from there, and when that one line was missing the whole
authored suite failed to load and therefore had never run — while looking, in the repository, exactly
like a suite that passed.

The BASE fixture is the negative half, and it is checked on every case by "the repaired tree is not
clean": a bare specifier must not be resolved, `export * from` must suppress the absence claim, an
`export { x as y }` alias must be matched on `y` and not on `x`, and a default import must not be
looked for in the namespace.
"""

from __future__ import annotations

import os
import re
import tempfile

import gate

BASE_SCAFFOLD = """\
import { browser } from "@wdio/globals";
export const page = { open: () => browser.url("/") };
function rawInvoke(cmd, args) { return browser.execute(cmd, args); }
export { rawInvoke as invoke };
export default page;
"""

# The real `lib/acceptance.js` re-exports by NAME and has no `export *` anywhere in the tree — so
# the splice pin lives in its own module rather than being folded in here, where it would have made
# case 1 unfireable: with a star in this file, deleting `export { invoke };` breaks nothing, because
# the star still re-exports it. (That is exactly what the first draft of this fixture did, and the
# case failed rather than passing for the wrong reason — which is the pair doing its job.)
BASE_ACCEPTANCE = """\
import fs from "node:fs";
import { invoke } from "../pages/scaffold.js";
export { invoke };
export async function installConsoleGuard() { return fs; }
"""

BASE_SPLICE = """\
export * from "../pages/scaffold.js";
"""

BASE_SPEC = """\
import { expect } from "@wdio/globals";
import scaffold, { page, invoke } from "../pages/scaffold.js";
import { invoke as acceptInvoke, installConsoleGuard } from "../lib/acceptance.js";
import { page as splicedPage } from "../lib/splice.js";

describe("a spec that loads", () => {
  it("runs", async () => {
    await page.open();
    expect(await invoke("noop")).toBeTruthy();
    await acceptInvoke("noop");
    await installConsoleGuard();
    await splicedPage.open();
    await scaffold.open();
  });
});
"""

FILES = {
    "editor-shell/e2e/pages/scaffold.js": BASE_SCAFFOLD,
    "editor-shell/e2e/lib/acceptance.js": BASE_ACCEPTANCE,
    "editor-shell/e2e/lib/splice.js": BASE_SPLICE,
    "editor-shell/e2e/specs-demo/demo.e2e.js": BASE_SPEC,
    "editor-shell/e2e/package.json": '{ "name": "e2e", "type": "module" }\n',
}


def build(tmp: str, overrides: dict[str, str] | None = None, drop: set[str] | None = None) -> str:
    root = os.path.join(tmp, "repo")
    files = dict(FILES)
    files.update(overrides or {})
    for rel in drop or set():
        files.pop(rel, None)
    for rel, text in files.items():
        path = os.path.join(root, *rel.split("/"))
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w", encoding="utf-8") as fh:
            fh.write(text)
    return root


def swap(src: str, old: str, new: str) -> str:
    """`str.replace`, except replacing nothing raises — a case whose drift silently became a no-op
    runs against an undrifted tree and proves nothing."""
    if old not in src:
        raise AssertionError(f"the fixture no longer contains {old[:80]!r}; the case is disarmed")
    return src.replace(old, new)


# (name, expect regex, drifted kwargs, repaired kwargs)
CASES: list[tuple[str, str, dict, dict]] = [
    (
        "a missing re-export — prompt 40's whole suite, which had never once run",
        r"imports `invoke` from \"\.\./lib/acceptance\.js\", which does not export it",
        {"overrides": {"editor-shell/e2e/lib/acceptance.js":
                       swap(BASE_ACCEPTANCE, "export { invoke };\n", "")}},
        {},
    ),
    (
        "an import of a module that is not there",
        r'imports from "\.\./pages/gone\.js", which does not exist',
        {"overrides": {"editor-shell/e2e/specs-demo/demo.e2e.js":
                       swap(BASE_SPEC, 'from "../pages/scaffold.js"', 'from "../pages/gone.js"')}},
        {},
    ),
    (
        "a spec that does not parse at all",
        r"does not parse — mocha would error before reaching an assertion",
        {"overrides": {"editor-shell/e2e/specs-demo/demo.e2e.js":
                       swap(BASE_SPEC, "describe(\"a spec that loads\", () => {",
                            "describe(\"a spec that loads\", () => { const = ;")}},
        {},
    ),
    (
        "a helper that stops exporting the name the specs import",
        r"imports `page` from \"\.\./pages/scaffold\.js\", which does not export it",
        {"overrides": {"editor-shell/e2e/pages/scaffold.js":
                       swap(BASE_SCAFFOLD, "export const page =", "const page =")}},
        {},
    ),
    (
        "the E2E tree has moved, so the gate has been checking nothing",
        r"the E2E tree does not exist at this path",
        {"drop": set(FILES)},
        {},
    ),
    (
        "the E2E tree is there and holds no spec at all",
        r"sweeps zero files passes every run and means nothing",
        {"drop": {k for k in FILES if k.endswith((".js", ".mjs"))}},
        {},
    ),
]


def _problems(tmp: str, spec: dict) -> list[gate.Problem]:
    root = build(tmp, spec.get("overrides"), spec.get("drop"))
    if spec.get("drop") == set(FILES):
        # Every file gone: `build` cannot create the directory, so point the gate at the root it
        # would have had. That IS the "the tree moved" condition, not a contrivance of it.
        os.makedirs(root, exist_ok=True)
    return gate.run(root)[0]


def run() -> int:
    ok = True
    for name, pattern, drifted, repaired in CASES:
        rx = re.compile(pattern, re.S)
        with tempfile.TemporaryDirectory() as tmp:
            got = _problems(tmp, drifted)
        with tempfile.TemporaryDirectory() as tmp:
            clean = _problems(tmp, repaired)

        hit = next((p for p in got if rx.search(p.message)), None)
        still = next((p for p in clean if rx.search(p.message)), None)
        if hit is None:
            print(f"FAIL  missed: {name}")
            for p in got:
                print(f"        got [{p.check}] {p.message[:130]}")
            ok = False
        elif still is not None:
            print(f"FAIL  fires on the REPAIRED tree too, so it proves nothing: {name}")
            ok = False
        elif clean:
            print(f"FAIL  the repaired tree is not clean: {name}")
            for p in clean:
                print(f"        got [{p.check}] {p.message[:130]}")
            ok = False
        else:
            print(f"pass  caught, and only when drifted: {name}")
            print(f"        → {hit.message[:140]}")

    print()
    if ok:
        print("self-test: every case is caught when drifted and absent when repaired, and the base "
              "fixture — a bare specifier, an `export *` splice, an `as` alias and a default import "
              "— stays clean throughout.")
    else:
        print("self-test: FAILED — the gate does not catch what it claims to catch.")
    return 0 if ok else 1


# See the identical block in `tools/ipc-contract-audit/selftest.py`. CI reaches this through
# `gate.py --self-test`; a human reaches for `python selftest.py`, and without this that invocation
# printed nothing and exited 0 — a self-test reporting success over zero cases, which is the one
# result a self-test must never be able to give.
if __name__ == "__main__":
    raise SystemExit(run())
