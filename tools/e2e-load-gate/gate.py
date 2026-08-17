#!/usr/bin/env python3
"""e2e-load-gate — would these 142 specs still LOAD, if anyone ran them?

`editor-shell/e2e` is driven by wdio against a packaged `.exe`, on a machine with a display, by a
human who remembered. Between those runs nothing opens the files at all: they are plain `.js` with
no `tsconfig`, vitest never loads them, and `cargo` has no idea they exist. The first thing that
ever evaluates a line here is mocha, hours into a run.

This gate is the cheapest thing that is nonetheless real. It answers two questions, at rest, in
about two seconds, with no display, no `.exe` and no `node_modules`:

  parse    does the file parse at all? (`node --check`, honouring the package's `"type": "module"`)
  imports  does every name a file imports from a RELATIVE module actually get exported by it?

The second is not hypothetical, and it is the reason this tool has the name it has. Prompt 40's
entire E2E suite was authored, committed, believed, and had **never once run**: a missing `invoke`
re-export in `lib/acceptance.js` made every spec fail to load, so all of them errored before mocha
reached an assertion. The suite "existed" for weeks and proved nothing, and that incident is now
orchestrator doctrine (`<verification_states_and_convergence>` (a): an authored-but-unrun gate is
not accepted). `lib/acceptance.js` still carries the `export { invoke };` line that closed it.

An import from a bare specifier (`@wdio/globals`, `node:fs`) is NOT checked: resolving those needs
an install, and a gate that needs an install is a gate that gets skipped. Their absence is loud
anyway — the runner fails immediately for everyone, on the first spec, rather than quietly for one
file nobody runs.

What this gate does NOT claim: parsing is not running, and a name that exists is not a name that
behaves. The TypeError class — a spec reading a reply field the shell stopped sending — is not
visible here at all; that is `tools/ipc-contract-audit`'s `reads` check, and the two are siblings
covering the same untyped tree from opposite ends.

Exit status is 1 if any check fails, so it can stand as a gate. A tree it cannot read fails too:
sweeping zero files is reported as a failure, never as a clean run.

Usage:  python3 gate.py [--root DIR] [--json] [--self-test]
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
from dataclasses import dataclass, asdict

HERE = os.path.dirname(os.path.abspath(__file__))
DEFAULT_ROOT = os.path.normpath(os.path.join(HERE, "..", ".."))

TREE = ("editor-shell", "e2e")
SKIP_DIRS = ("node_modules", "baselines", "fixtures", "samples", "shots", "screenshots")


@dataclass
class Problem:
    check: str  # "parse" | "imports" | "coverage"
    where: str
    message: str


# ── reading the module graph ──────────────────────────────────────────────────────────────────────

_IMPORT = re.compile(
    r"""import\s+(?P<clause>[^;'"]*?)\s*from\s*["'](?P<spec>[^"']+)["']""", re.S
)
_EXPORT_FROM = re.compile(r"""export\s*\*\s*(?:as\s+\w+\s*)?from\s*["'](?P<spec>[^"']+)["']""")
_IDENT = r"[A-Za-z_$][A-Za-z0-9_$]*"


def strip_comments(src: str) -> str:
    """Drop comments, preserving line count so reported lines stay true. A `//` inside a string
    (`"https://…"`) is not a comment, and a gate that treats it as one starts reading code as prose."""
    out: list[str] = []
    i, n = 0, len(src)
    while i < n:
        c = src[i]
        if c in "\"'`":
            j = i + 1
            while j < n and src[j] != c:
                j += 2 if src[j] == "\\" else 1
            out.append(src[i : j + 1])
            i = j + 1
            continue
        if src.startswith("/*", i):
            j = src.find("*/", i + 2)
            j = n if j < 0 else j + 2
            out.append("\n" * src.count("\n", i, j))
            i = j
            continue
        if src.startswith("//", i):
            j = src.find("\n", i)
            i = n if j < 0 else j
            continue
        out.append(c)
        i += 1
    return "".join(out)


def named_imports(clause: str) -> list[str]:
    """The names taken out of a module's namespace by one import clause.

    `import def, { a, b as c } from ".."` yields `a` and `b` — the LOCAL alias is irrelevant, what
    must exist is the exported name. A default import and `* as ns` are not names in the namespace,
    so they are not checked here.
    """
    m = re.search(r"\{(?P<inner>[^}]*)\}", clause)
    if not m:
        return []
    out = []
    for part in m.group("inner").split(","):
        part = part.strip()
        if not part:
            continue
        name = part.split(" as ")[0].strip()
        if re.fullmatch(_IDENT, name):
            out.append(name)
    return out


def exported_names(src: str) -> tuple[set[str], bool]:
    """(names this module exports, whether it re-exports a whole other module).

    The `export *` flag matters: a module that splices in another's namespace can export a name this
    reader cannot enumerate, so absence stops being provable — the same rule the IPC audit applies
    to `#[serde(flatten)]`, for the same reason.
    """
    names: set[str] = set()
    for m in re.finditer(
        rf"export\s+(?:async\s+)?(?:function\s*\*?|class|const|let|var)\s+({_IDENT})", src
    ):
        names.add(m.group(1))
    # `export { a, b as c }` — the EXPORTED name is what an importer asks for, so `c`, not `b`.
    for m in re.finditer(r"export\s*\{(?P<inner>[^}]*)\}", src):
        for part in m.group("inner").split(","):
            part = part.strip()
            if not part:
                continue
            name = part.split(" as ")[-1].strip()
            if re.fullmatch(_IDENT, name):
                names.add(name)
    if re.search(r"export\s+default\b", src):
        names.add("default")
    return names, bool(_EXPORT_FROM.search(src))


def sweep(root: str) -> list[tuple[str, str]]:
    base = os.path.join(root, *TREE)
    out: list[tuple[str, str]] = []
    for dirpath, dirnames, filenames in os.walk(base):
        dirnames[:] = sorted(d for d in dirnames if d not in SKIP_DIRS)
        for fn in sorted(filenames):
            if not fn.endswith((".js", ".mjs")):
                continue
            path = os.path.join(dirpath, fn)
            with open(path, encoding="utf-8", errors="replace") as fh:
                out.append((path, fh.read()))
    return out


# ── the checks ────────────────────────────────────────────────────────────────────────────────────


def check_parse(files: list[tuple[str, str]], root: str) -> list[Problem]:
    """`node --check` on every file. Node's absence is a FAILURE, not a skip: a gate that silently
    stops checking when a tool is missing reports agreement about files it never opened, which is
    precisely how the GPU audit went blind (`progress/gpu-contract-audit/discovery-blind-spot.md`)."""
    try:
        subprocess.run(["node", "--version"], capture_output=True, check=True)
    except (OSError, subprocess.CalledProcessError):
        return [Problem("coverage", "<node>", "`node` is not on PATH, so NOTHING was parsed. This "
                                              "gate reports what it read; it did not read anything")]
    out: list[Problem] = []
    for path, _ in files:
        r = subprocess.run(["node", "--check", path], capture_output=True, text=True)
        if r.returncode != 0:
            first = next((ln.strip() for ln in r.stderr.splitlines()
                          if ln.strip() and not ln.strip().startswith("at ")), r.stderr.strip())
            out.append(Problem("parse", rel(path, root),
                               f"does not parse — mocha would error before reaching an "
                               f"assertion: {first[:200]}"))
    return out


def check_imports(files: list[tuple[str, str]], root: str) -> tuple[list[Problem], int]:
    out: list[Problem] = []
    sources = {os.path.normpath(p): strip_comments(s) for p, s in files}
    checked = 0
    for path, raw in files:
        src = strip_comments(raw)
        for m in _IMPORT.finditer(src):
            spec = m.group("spec")
            if not spec.startswith("."):
                continue  # a bare specifier needs an install to resolve; see the module docstring
            target = os.path.normpath(os.path.join(os.path.dirname(path), spec))
            line = src.count("\n", 0, m.start()) + 1
            if not os.path.isfile(target):
                out.append(Problem("imports", f"{rel(path, root)}:{line}",
                                   f'imports from "{spec}", which does not exist — this file cannot '
                                   "load, and every test in it errors before it runs"))
                continue
            if target not in sources:
                with open(target, encoding="utf-8", errors="replace") as fh:
                    sources[target] = strip_comments(fh.read())
            exports, splices = exported_names(sources[target])
            if splices:
                continue  # `export * from ..` splices in names this reader cannot enumerate
            for name in named_imports(m.group("clause")):
                checked += 1
                if name not in exports:
                    out.append(Problem(
                        "imports", f"{rel(path, root)}:{line}",
                        f'imports `{name}` from "{spec}", which does not export it '
                        f"({rel(target, root)} exports {sorted(exports) or 'nothing'}) — the module "
                        "fails to load, so every test in this file errors without running. This is "
                        "the prompt-40 incident exactly: a missing re-export made an entire "
                        "authored suite silently never run"))
    return out, checked


def rel(path: str, root: str) -> str:
    return os.path.relpath(path, root).replace(os.sep, "/")


def run(root: str) -> tuple[list[Problem], dict]:
    base = os.path.join(root, *TREE)
    if not os.path.isdir(base):
        return ([Problem("coverage", "/".join(TREE),
                         "the E2E tree does not exist at this path. Either it moved — in which case "
                         "this gate has been checking nothing — or the root is wrong")],
                {"files": 0, "names_checked": 0})
    files = sweep(root)
    if not files:
        return ([Problem("coverage", "/".join(TREE),
                         "the E2E tree exists and contains no .js at all. A gate that sweeps zero "
                         "files passes every run and means nothing")],
                {"files": 0, "names_checked": 0})
    problems = check_parse(files, root)
    imp, checked = check_imports(files, root)
    problems += imp
    problems.sort(key=lambda p: ({"coverage": 0, "parse": 1, "imports": 2}[p.check], p.where))
    return problems, {"files": len(files), "names_checked": checked}


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--root", default=DEFAULT_ROOT)
    ap.add_argument("--json", action="store_true")
    ap.add_argument("--self-test", action="store_true")
    a = ap.parse_args()

    if a.self_test:
        import selftest

        return selftest.run()

    problems, stats = run(a.root)
    if a.json:
        print(json.dumps({"stats": stats, "problems": [asdict(p) for p in problems]}, indent=2))
        return 1 if problems else 0

    print(f"e2e-load-gate — {stats['files']} spec file(s) parsed and "
          f"{stats['names_checked']} imported name(s) resolved against the module that exports them")
    for p in problems:
        print(f"  ERROR  [{p.check:<8}] {p.where}\n           {p.message}")
    if not problems:
        print("\n  every spec parses, and every name imported from a relative module is one that "
              "module exports — so no spec fails before mocha reaches its first assertion.")
    else:
        print(f"\n  {len(problems)} blocking.")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
