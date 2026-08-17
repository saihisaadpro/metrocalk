#!/usr/bin/env python3
"""ipc-contract-audit — does the TypeScript still ask for the command the Rust still answers?

The editor is two programs. `editor-shell/src-tauri` declares the commands; `editor/src` calls them.
Between the two there is no type system, no linker and no compiler: Tauri dispatches on a **string**
the TypeScript hands it, deserialises an argument object by **key name**, and hands back JSON the
TypeScript **asserts** a type for. `cargo` checks one side. `tsc` checks the other. Nothing checks
the pair — the first thing that compares them is a user, clicking, in the packaged `.exe`.

The three ways that goes wrong are not equally loud, and the quietest is the worst:

  * a **name** drifts — `invoke` rejects, and something in the UI stops working with a console error;
  * an **argument key** drifts — the command rejects the payload, same class of loud failure;
  * a **reply field** drifts — JavaScript reads `undefined`. Nothing throws. The panel renders blank
    and every test that never opened that panel stays green.

This repository has been caught by this boundary twice, and both are written into the orchestrator
as doctrine: prompt 40's E2E suite that had **never actually run** because an `invoke` re-export was
missing, and finding **C6** — panels that were green against the dev MockCore and surfaced **zero**
rows against the real `/core`. The Vitest suite cannot catch either: the dev path uses an in-process
client that never calls `invoke` at all, so every call site here is dark until the `.exe` runs.

This runs the comparison at rest, with no toolchain, no WebView and no GPU:

  registration  every `#[tauri::command]` reaches `generate_handler!` — an unregistered command is
                a name that does not exist, however correct its body
  command       every `invoke("name")` names a command the shell registers
  arguments     every required argument is sent, and every key sent is one the command accepts
                (under Tauri's camelCase argument convention)
  shape         every field the caller's `T` requires is one the reply actually carries, a list is
                a list on both sides, and a fixed-length tuple has the same length on both. TOP
                LEVEL ONLY: a nested object's fields are not walked, and a field typed
                `serde_json::Value` is reported by `coverage` rather than counted as agreed
  coverage      the audit's own reach — see `_coverage_findings`

Known and deliberate limits, so that "0 blocking" is read for what it is: argument STRUCTS
(`#[derive(Deserialize)]` payloads handed over inside one key) are not parsed, nullability is not
compared (`Option<T>` still puts the key on the wire), and Rust enums are not read. Each is a place
a future check goes, not a place this one silently claims success.

Exit status is 1 if any check fails, so it can stand as a gate. Anything it could not read is
reported and fails too: this gate's predecessor spent a day reporting agreement about files it had
never opened (`progress/gpu-contract-audit/discovery-blind-spot.md`), and the lesson generalises —
**a checker that stops looking fails silently forever, and takes every future drift with it.**

Usage:  python3 audit.py [--root DIR] [--json] [--no-waivers] [--self-test]
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
from dataclasses import dataclass, asdict

import rustipc
import tsipc

HERE = os.path.dirname(os.path.abspath(__file__))
DEFAULT_ROOT = os.path.normpath(os.path.join(HERE, "..", ".."))

# The two sides, relative to the repository root. Written as path components so the same list works
# on Windows, where this app is actually built and where CI runs it.
#
# Every REQUIRED file must exist: a renamed side of the contract must fail the gate, not shrink it.
# That is the specific way the GPU audit went blind — its glob quietly stopped matching the renderer,
# and the only trace was a smaller number in a header line nobody diffs.
REQUIRED_RUST = (
    ("editor-shell", "src-tauri", "src", "main.rs"),  # the commands and `generate_handler!`
    ("editor-shell", "src", "lib.rs"),  # the shell library that declares most reply DTOs
)
# **Recursive on purpose**, and swept for DTOs in addition to the required files above. Half the
# reply types (`RoleReply`, `PipeForgeStatus`, `ShapeSpec`, …) live in the `editor-shell` library
# crate, not beside the command that returns them. Reading only the command file left 46 of 202 call
# sites with an unresolvable reply — every one of which the `shape` check would have skipped, which
# is the same output as agreement.
RUST_TREES = (
    ("editor-shell", "src"),
    ("editor-shell", "src-tauri", "src"),
    ("core", "src"),  # `CatalogSearch` and friends: a reply type can come from any crate the shell re-exports
)
# Files that CALL invoke. Splitting this from the type sources keeps a stray `.invoke(` in a test
# helper from being read as production UI.
TS_CALL_SOURCES = (("editor", "src", "transport", "session.ts"),)
# The E2E suite talks to the SAME command surface, by string, and it is five times the size of the
# production transport. It is also the exact scene of this gate's founding incident: prompt 40's
# suite had never actually run, so nothing in it was known to be true. A spec that invokes a renamed
# command fails on the `.exe`, hours into a run, on a machine with a display — or, if that spec is
# one of the ones nobody runs, never. Swept recursively, and REQUIRED to be non-empty for the same
# reason the Rust trees are.
TS_CALL_TREES = (("editor-shell", "e2e"),)
TS_CALL_TREE_SKIP = ("node_modules", "baselines", "fixtures", "samples", "shots")
# Files that DECLARE the shapes `invoke<T>` refers to.
TS_TYPE_SOURCES = (
    ("editor", "src", "transport", "protocol.ts"),
    ("editor", "src", "transport", "session.ts"),
    ("editor", "src", "store", "project.ts"),
    ("editor", "src", "store", "play.ts"),
)

WAIVERS = os.path.join(HERE, "waivers.txt")


@dataclass
class Finding:
    severity: str  # "error" | "unresolved"
    check: str
    where: str
    message: str
    #: What a waiver line names. A line number would expire on the next edit above it and a bare
    #: filename would waive every finding in a 26,000-line file at once, so the key is the thing the
    #: finding is ABOUT — a command, a call site's command, a path.
    key: str = ""

    def __post_init__(self) -> None:
        if not self.key:
            self.key = f"{self.check}@{self.where.split(':')[0]}"


# ── how a Rust return type maps onto JSON ─────────────────────────────────────────────────────────

_SCALAR = {
    "String": "string",
    "str": "string",
    "bool": "boolean",
    "char": "string",
    "PathBuf": "string",
    **{n: "number" for n in
       ("f32", "f64", "u8", "u16", "u32", "u64", "u128", "usize",
        "i8", "i16", "i32", "i64", "i128", "isize", "NonZeroU32", "NonZeroUsize")},
}
_TRANSPARENT = ("Option", "Result", "Vec", "VecDeque", "Box", "Arc", "Rc", "Cow")


def _rust_tuple_arity(t: str) -> int | str:
    """`[f32; 6]` -> 6; `(String, bool)` -> 2; anything unreadable -> "?" (never a number)."""
    t = t.strip()
    if t.startswith("["):
        m = re.match(r"^\[.*;\s*(\d+)\s*\]$", t)
        return int(m.group(1)) if m else "?"
    if t.startswith("("):
        return len(rustipc.split_top(t[1:-1]))
    return "?"


def _ts_tuple_arity(t: str) -> int | str:
    t = t.strip()
    if t.startswith("[") and t.endswith("]"):
        return len(tsipc.split_top(t[1:-1]))
    return "?"


#: TS reads that assert nothing, so they cannot conflict with anything.
_TS_PERMISSIVE = frozenset({"structural", "unknown", "any", "Json", "void", "null"})


def _scalar_conflict(rust: str, ts: str) -> str:
    """Why a non-object reply and the type read from it disagree, or "" when they can agree.

    Deliberately asymmetric: a LOOSER read is fine and a WRONG one is not. `invoke<number[]>` over a
    Rust `[f64; 8]` simply declines to assert the length, which is a choice, not a drift. A TS tuple
    of a different length, or a `number` where the shell sends a string, is a drift.
    """
    if ts in _TS_PERMISSIVE or rust in ("map", "<tuple:?>"):
        return ""
    r_tuple, t_tuple = rust.startswith("<tuple:"), ts.startswith("<tuple:")
    if r_tuple and t_tuple:
        rn, tn = rust[7:-1], ts[7:-1]
        if "?" in (rn, tn) or rn == tn:
            return ""
        return f"reads a {tn}-element tuple where the reply has {rn} elements"
    if r_tuple and not t_tuple:
        return ""  # `number[]` over a fixed-size array: looser, not wrong
    if t_tuple and not r_tuple:
        return f"reads a fixed-length tuple, but the reply is {rust}"
    if rust == ts:
        return ""
    return f"reads {ts} where the reply is {rust}"


def rust_shape(ret: str, dtos: dict[str, rustipc.Dto]) -> tuple[object, bool, bool]:
    """(shape, is_list, resolved).

    `shape` is a `Dto` for an object, a str for a scalar/opaque kind, or None. `resolved` is False
    when the reader could not decide — never conflated with "it is an empty object", because those
    two produce identical output and only one of them is agreement.
    """
    t = ret.strip()
    is_list = False
    for _ in range(6):
        head = rustipc.type_head(t)
        if head in ("Vec", "VecDeque") :
            t = t[t.index("<") + 1 : t.rindex(">")].strip()
            is_list = True
            continue
        if head in ("Option", "Box", "Arc", "Rc", "Cow"):
            inner = t[t.index("<") + 1 : t.rindex(">")].strip()
            t = rustipc.split_top(inner)[-1] if head == "Cow" else inner
            continue
        if head == "Result":
            t = rustipc.split_top(t[t.index("<") + 1 : t.rindex(">")])[0].strip()
            continue
        break
    head = rustipc.type_head(t)
    if head in ("[]", "(,)"):
        # `[f32; 3]` and `(String, bool)` both serialise as a JSON array of FIXED length, so the
        # length is part of the contract. Collapsing both to a bare "it is a list" let a Rust
        # 5-tuple and a TS 3-tuple compare equal — "compared" with nothing compared.
        return f"<tuple:{_rust_tuple_arity(t)}>", True, True
    if head == "()":
        return "void", False, True
    if head in _SCALAR:
        return _SCALAR[head], is_list, True
    if head in ("BTreeMap", "HashMap"):
        return "map", is_list, True
    if head == "Value":
        return None, is_list, False  # an untyped reply: no static shape exists on this side
    if t in dtos:
        return dtos[t], is_list, True
    if head in dtos:
        return dtos[head], is_list, True
    return None, is_list, False


def ts_shape(
    targ: str, types: dict[str, tsipc.TsType], aliases: dict[str, str] | None = None
) -> tuple[object, bool, bool]:
    base, is_list, _ = tsipc.unwrap(targ, aliases)
    inline = tsipc.object_literal_fields(base)
    if inline is not None:
        return tsipc.TsType("<inline>", inline, 0, ""), is_list, True
    if base in ("unknown", "any", "void", "Json", "null") or base.startswith("<tuple:"):
        return base, is_list, True
    if base in ("string", "number", "boolean"):
        return base, is_list, True
    if base.startswith("Record<") or base.startswith("Partial<") or base.startswith("{"):
        return "structural", is_list, True
    if base in types:
        return types[base], is_list, True
    if re.fullmatch(r'"[^"]*"(\s*\|\s*"[^"]*")*', base):
        return "string", is_list, True
    return None, is_list, False


# ── the checks ────────────────────────────────────────────────────────────────────────────────────


def _registration_findings(rs: rustipc.RustIpc) -> list[Finding]:
    out: list[Finding] = []
    registered = set(rs.registered)
    for name, c in sorted(rs.commands.items()):
        if c.unreadable or not c.name:
            continue
        if name not in registered:
            out.append(
                Finding(
                    "error",
                    "registration",
                    f"{c.file}:{c.line}",
                    f"`{name}` is a #[tauri::command] that `generate_handler!` never lists, so the "
                    "name does not exist at run time — invoking it is rejected however correct the body",
                )
            )
    return out


def _command_findings(rs: rustipc.RustIpc, ts: tsipc.TsIpc) -> list[Finding]:
    out: list[Finding] = []
    registered = set(rs.registered)
    for inv in ts.invocations:
        if inv.cmd is None:
            continue
        if inv.cmd not in registered:
            near = ""
            close = [r for r in registered if r.replace("_", "") == inv.cmd.replace("_", "")]
            if close:
                near = f" (the shell registers `{close[0]}`)"
            out.append(
                Finding(
                    "error",
                    "command",
                    f"{inv.file}:{inv.line}",
                    f'invoke("{inv.cmd}") names a command the shell does not register{near} — this '
                    "call rejects at run time, and only in the packaged app",
                )
            )
    return out


def _argument_findings(rs: rustipc.RustIpc, ts: tsipc.TsIpc) -> list[Finding]:
    out: list[Finding] = []
    for inv in ts.invocations:
        if inv.cmd is None or inv.cmd not in rs.commands:
            continue
        cmd = rs.commands[inv.cmd]
        if cmd.unreadable:
            continue
        accepted = {k for k, _, _ in cmd.args}
        required = {k for k, _, opt in cmd.args if not opt}
        sent = set(inv.keys)
        where = f"{inv.file}:{inv.line}"
        for k in sorted(sent - accepted):
            out.append(
                Finding(
                    "error",
                    "arguments",
                    where,
                    f'invoke("{inv.cmd}") sends `{k}`, which the command does not accept '
                    f"({cmd.file}:{cmd.line} takes {sorted(accepted) or 'no arguments'})",
                )
            )
        if inv.spread:
            continue  # the key set is not fully visible, so absence proves nothing
        for k in sorted(required - sent):
            out.append(
                Finding(
                    "error",
                    "arguments",
                    where,
                    f'invoke("{inv.cmd}") does not send `{k}`, which the command requires '
                    f"({cmd.file}:{cmd.line}) — the payload fails to deserialise",
                )
            )
    return out


def _shape_findings(rs: rustipc.RustIpc, ts: tsipc.TsIpc) -> tuple[list[Finding], int, int]:
    """Returns the findings and, so the header can state the check's REACH, how many replies were
    actually compared and how many of those got a field-by-field comparison rather than only a
    kind/list one. A `shape` line that says nothing is the output of both "they agree" and "I never
    resolved either side", and only the header can tell a reader which."""
    out: list[Finding] = []
    compared = fields_compared = 0
    for inv in ts.invocations:
        if inv.cmd is None or inv.targ is None or inv.cmd not in rs.commands:
            continue
        cmd = rs.commands[inv.cmd]
        if cmd.unreadable:
            continue
        r_shape, r_list, r_ok = rust_shape(cmd.ret, rs.dtos)
        t_shape, t_list, t_ok = ts_shape(inv.targ, ts.types, ts.aliases)
        where = f"{inv.file}:{inv.line}"
        if not (r_ok and t_ok):
            continue  # reported by coverage, not silently dropped
        compared += 1
        key = f"shape@{inv.cmd}"
        if r_shape == "void" and inv.targ not in ("void", "unknown"):
            out.append(
                Finding(
                    "error",
                    "shape",
                    where,
                    f'invoke<{inv.targ}>("{inv.cmd}") reads a reply, but the command returns nothing '
                    f"({cmd.file}:{cmd.line}) — the caller receives `null`",
                    key,
                )
            )
            continue
        if r_list != t_list:
            out.append(
                Finding(
                    "error",
                    "shape",
                    where,
                    f'invoke<{inv.targ}>("{inv.cmd}") expects '
                    f"{'a list' if t_list else 'a single value'}, but the command returns "
                    f"`{cmd.ret}` ({cmd.file}:{cmd.line})",
                    key,
                )
            )
            continue
        if not isinstance(t_shape, tsipc.TsType):
            # Not an object on the TS side — but "not an object" is not "nothing to check". A scalar
            # kind and a tuple's LENGTH are both part of the contract, and skipping straight to
            # `continue` here counted 51 replies as "compared" having compared nothing at all.
            if isinstance(r_shape, rustipc.Dto):
                out.append(
                    Finding(
                        "error", "shape", where,
                        f'invoke<{inv.targ}>("{inv.cmd}") reads a {t_shape}, but the command returns '
                        f"the object `{r_shape.name}` ({r_shape.file}:{r_shape.line})",
                        key,
                    )
                )
            elif isinstance(r_shape, str) and isinstance(t_shape, str):
                why = _scalar_conflict(r_shape, t_shape)
                if why:
                    out.append(
                        Finding(
                            "error", "shape", where,
                            f'invoke<{inv.targ}>("{inv.cmd}") {why}, but the command returns '
                            f"`{cmd.ret}` ({cmd.file}:{cmd.line})",
                            key,
                        )
                    )
            continue
        if not isinstance(r_shape, rustipc.Dto):
            out.append(
                Finding(
                    "error",
                    "shape",
                    where,
                    f'invoke<{inv.targ}>("{inv.cmd}") reads an object, but the command returns '
                    f"`{cmd.ret}`, which serialises as {r_shape} ({cmd.file}:{cmd.line})",
                    key,
                )
            )
            continue
        fields_compared += 1
        carried = {k for k, _ in r_shape.fields}
        if r_shape.collides_with:
            out.append(
                Finding(
                    "unresolved", "coverage", where,
                    f'"{inv.cmd}" returns `{r_shape.name}`, and two different structs in the swept '
                    f"trees carry that bare name ({r_shape.file}:{r_shape.line} and "
                    f"{r_shape.collides_with}). The reader keys DTOs by name, so it may be comparing "
                    "against the wrong one — disambiguate before trusting this reply's verdict",
                    key,
                )
            )
            continue
        # A field typed `serde_json::Value` IS on the wire, so the presence check below passes and
        # the reply counts as compared — while whatever the caller reads out of that field is not
        # checked at all. This is the waived top-level defect one level down, and being counted as
        # covered is precisely what makes it worth naming.
        opaque_read = [
            k for k, _ in t_shape.fields
            if k in r_shape.opaque
        ]
        if opaque_read:
            out.append(
                Finding(
                    "unresolved", "coverage", where,
                    f'"{inv.cmd}" replies with `{r_shape.name}`, whose {opaque_read} '
                    f"{'is' if len(opaque_read) == 1 else 'are'} typed `serde_json::Value` "
                    f"({r_shape.file}:{r_shape.line}) while `{t_shape.name}` reads a declared shape "
                    "out of it. The key is present, so this reply passes the field check — but "
                    "everything INSIDE that key is unchecked",
                    key,
                )
            )
        if any(k.startswith("<flatten:") for k in carried):
            continue  # a flattened field splices in keys this reader cannot enumerate
        missing = [k for k, opt in t_shape.fields if not opt and k not in carried]
        if missing:
            origin = "the json! literal it builds" if r_shape.origin == "json!" else f"`{r_shape.name}`"
            out.append(
                Finding(
                    "error",
                    "shape",
                    where,
                    f'invoke<{inv.targ}>("{inv.cmd}") requires {missing}, which {origin} never sends '
                    f"({r_shape.file}:{r_shape.line} carries {sorted(carried)}) — JavaScript reads "
                    "`undefined` and renders blank, throwing nothing",
                    key,
                )
            )
    return out, compared, fields_compared


def _coverage_findings(
    root: str,
    missing_files: list[str],
    rs: rustipc.RustIpc,
    ts: tsipc.TsIpc,
) -> list[Finding]:
    """Findings about the audit's own reach — the ones that keep "clean" from meaning "invisible".

    Every other check compares two statements of a contract. These compare the audit against the
    tree. Its predecessor had to learn this the expensive way: pointed at a directory that did not
    exist, `gpu-contract-audit` printed its success sentence and exited 0 — the same bytes for "I
    checked and it agrees" as for "I never looked". A gate sitting in CI in that state is worse than
    no gate, because it is *read* as the strict one.
    """
    out: list[Finding] = []
    if not os.path.isdir(root):
        return [
            Finding(
                "error",
                "coverage",
                root.replace(os.sep, "/"),
                "the audit root does not exist, so nothing was compared. This is a broken invocation "
                "reported as a clean gate — fix --root",
            )
        ]
    for rel in missing_files:
        out.append(
            Finding(
                "error",
                "coverage",
                rel,
                "this file is on the audit's source list and is not in the tree. A moved or renamed "
                "side of the contract must fail the gate, not quietly shrink it",
            )
        )
    if missing_files:
        return out  # every finding below would be vacuously absent

    if not rs.commands:
        out.append(
            Finding(
                "error",
                "coverage",
                "/".join(REQUIRED_RUST[0]),
                "no #[tauri::command] was found anywhere — a command surface the gate cannot see is "
                "not a command surface the gate agrees with",
            )
        )
    if not rs.handler_found:
        out.append(
            Finding(
                "error",
                "coverage",
                "/".join(REQUIRED_RUST[0]),
                "no `generate_handler![..]` was found, so the audit does not know which commands are "
                "registered and the `registration` check has no ground truth",
            )
        )
    if not ts.invocations:
        out.append(
            Finding(
                "error",
                "coverage",
                "/".join(TS_CALL_SOURCES[0]),
                "no `invoke(..)` call site was found — either the UI stopped calling the shell, or "
                "the transport moved and every call site is now unaudited",
            )
        )
    for key, c in sorted(rs.commands.items()):
        if c.unreadable:
            out.append(
                Finding(
                    "unresolved",
                    "coverage",
                    f"{c.file}:{c.line}",
                    f"a #[tauri::command] the audit could not read: {c.unreadable}. This command is "
                    "UNREADABLE to the gate, not clean",
                )
            )
    parsed = {n for n, c in rs.commands.items() if not c.unreadable}
    for name in sorted(set(rs.registered) - parsed):
        out.append(
            Finding(
                "unresolved",
                "coverage",
                "generate_handler!",
                f"`{name}` is registered, but the audit found no `#[tauri::command] fn {name}` — the "
                "handler compiles, so the parser missed it. Teach rustipc.py the shape it uses",
            )
        )
    # Aggregated ON PURPOSE, into exactly one finding. The E2E suite drives the shell through
    # `browser.execute((c, a) => ..invoke(c, a))` helpers, where the name is a runtime value — an
    # honest limit, not a defect, and 37 identical lines would be noise that trains a reader to skim
    # past the section. One line keeps the COUNT visible instead: if it jumps, coverage fell.
    computed = [inv for inv in ts.invocations if inv.cmd is None]
    if computed:
        files = sorted({inv.file for inv in computed})
        out.append(
            Finding(
                "unresolved",
                "coverage",
                files[0] if len(files) == 1 else f"{len(files)} file(s)",
                f"{len(computed)} call site(s) pass the command name as a variable "
                f"(e.g. {files[0]}:{computed[0].line} `{computed[0].raw}`), so no name, argument or "
                "shape check is possible at any of them. This number is the size of the gate's blind "
                "spot: if it grows, coverage shrank",
                key="coverage@computed-command-names",
            )
        )
    # An unreadable TYPE is a hole in the shape check, and a hole that reads as agreement. Split by
    # side so the reader is told which half to repair.
    for inv in ts.invocations:
        if inv.cmd is None or inv.targ is None or inv.cmd not in rs.commands:
            continue
        cmd = rs.commands[inv.cmd]
        if cmd.unreadable:
            continue
        _, _, t_ok = ts_shape(inv.targ, ts.types, ts.aliases)
        if not t_ok:
            out.append(
                Finding(
                    "unresolved",
                    "coverage",
                    f"{inv.file}:{inv.line}",
                    f"`{inv.targ}` is not declared in any file on the audit's TS source list, so the "
                    f'reply shape of "{inv.cmd}" is unchecked. Add the file that declares it',
                    key=f"shape@{inv.cmd}",
                )
            )
            continue
        _, _, r_ok = rust_shape(cmd.ret, rs.dtos)
        if not r_ok:
            out.append(
                Finding(
                    "unresolved",
                    "coverage",
                    f"{inv.file}:{inv.line}",
                    f'"{inv.cmd}" returns `{cmd.ret}` ({cmd.file}:{cmd.line}), which has no field '
                    f"names to compare, while the caller reads `{inv.targ}` as though it did. This "
                    "reply is UNCHECKED, not agreed: give the command a named #[derive(Serialize)] "
                    "type, or build it with one `serde_json::json!({..})` this reader can see",
                    key=f"shape@{inv.cmd}",
                )
            )
    return out


# ── waivers ───────────────────────────────────────────────────────────────────────────────────────


def load_waivers(path: str) -> dict[str, str]:
    out: dict[str, str] = {}
    if not os.path.exists(path):
        return out
    with open(path, encoding="utf-8") as fh:
        for ln in fh:
            ln = ln.strip()
            if not ln or ln.startswith("#") or ":" not in ln:
                continue
            k, reason = ln.split(":", 1)
            out[k.strip()] = reason.strip()
    return out


def waiver_key(f: Finding) -> str:
    return f.key


# ── run ───────────────────────────────────────────────────────────────────────────────────────────


def read_sources(root: str, specs) -> tuple[list[tuple[str, str]], list[str]]:
    found: list[tuple[str, str]] = []
    missing: list[str] = []
    for parts in specs:
        rel = "/".join(parts)
        path = os.path.join(root, *parts)
        if not os.path.isfile(path):
            missing.append(rel)
            continue
        with open(path, encoding="utf-8", errors="replace") as fh:
            found.append((rel, fh.read()))
    return found, missing


def sweep_rust(root: str) -> list[tuple[str, str]]:
    """Every `.rs` under the declared trees, recursively, sorted so the run is reproducible."""
    out: list[tuple[str, str]] = []
    for parts in RUST_TREES:
        base = os.path.join(root, *parts)
        for dirpath, dirnames, filenames in os.walk(base):
            dirnames[:] = sorted(d for d in dirnames if d not in ("target", "node_modules"))
            for fn in sorted(filenames):
                if not fn.endswith(".rs"):
                    continue
                path = os.path.join(dirpath, fn)
                rel = os.path.relpath(path, root).replace(os.sep, "/")
                with open(path, encoding="utf-8", errors="replace") as fh:
                    out.append((rel, fh.read()))
    return out


def sweep_ts_calls(root: str) -> list[tuple[str, str]]:
    out: list[tuple[str, str]] = []
    for parts in TS_CALL_TREES:
        base = os.path.join(root, *parts)
        for dirpath, dirnames, filenames in os.walk(base):
            dirnames[:] = sorted(d for d in dirnames if d not in TS_CALL_TREE_SKIP)
            for fn in sorted(filenames):
                if not fn.endswith((".ts", ".tsx", ".js", ".mjs")):
                    continue
                path = os.path.join(dirpath, fn)
                rel = os.path.relpath(path, root).replace(os.sep, "/")
                with open(path, encoding="utf-8", errors="replace") as fh:
                    out.append((rel, fh.read()))
    return out


def run(root: str) -> tuple[list[Finding], dict]:
    _, rust_missing = read_sources(root, REQUIRED_RUST)
    ts_call_src, ts_call_missing = read_sources(root, TS_CALL_SOURCES)
    ts_type_src, ts_type_missing = read_sources(root, TS_TYPE_SOURCES)
    missing = rust_missing + ts_call_missing + [m for m in ts_type_missing if m not in ts_call_missing]
    rust_src = sweep_rust(root) if os.path.isdir(root) else []
    e2e_src = sweep_ts_calls(root) if os.path.isdir(root) else []

    rs = rustipc.parse(rust_src)
    seen: dict[str, str] = {}
    for rel, text in ts_call_src + ts_type_src + e2e_src:
        seen[rel] = text
    call_files = {"/".join(p) for p in TS_CALL_SOURCES} | {rel for rel, _ in e2e_src}
    ts = tsipc.parse(list(seen.items()), call_files)

    findings = _coverage_findings(root, missing, rs, ts)
    compared = fields_compared = 0
    if not any(f.check == "coverage" and "audit root does not exist" in f.message for f in findings) \
            and not missing:
        findings += _registration_findings(rs)
        findings += _command_findings(rs, ts)
        findings += _argument_findings(rs, ts)
        shape, compared, fields_compared = _shape_findings(rs, ts)
        findings += shape

    stats = {
        "rust_files": len(rust_src),
        "rust_required_present": len(REQUIRED_RUST) - len(rust_missing),
        "rust_required": len(REQUIRED_RUST),
        "ts_files": len(seen),
        "ts_files_expected": len({"/".join(p) for p in TS_CALL_SOURCES + TS_TYPE_SOURCES}),
        "commands": len([c for c in rs.commands.values() if not c.unreadable]),
        "registered": len(rs.registered),
        "invocations": len(ts.invocations),
        "typed_invocations": len([i for i in ts.invocations if i.targ]),
        "shape_compared": compared,
        "shape_fields": fields_compared,
        "dtos": len(rs.dtos),
        "ts_types": len(ts.types),
    }
    order = {"registration": 0, "command": 1, "arguments": 2, "shape": 3, "coverage": 4}
    findings.sort(key=lambda f: (order.get(f.check, 9), f.where, f.message))
    return findings, stats


SUCCESS = (
    "every invoked command exists, every argument key is accepted, and every top-level reply field "
    "the UI reads is one the shell sends."
)


def main() -> int:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    ap.add_argument("--root", default=DEFAULT_ROOT, help="repository root")
    ap.add_argument("--json", action="store_true")
    ap.add_argument("--no-waivers", action="store_true")
    ap.add_argument("--self-test", action="store_true")
    a = ap.parse_args()

    if a.self_test:
        import selftest

        return selftest.run()

    findings, stats = run(a.root)
    waivers = {} if a.no_waivers else load_waivers(WAIVERS)

    if a.json:
        print(json.dumps(
            {"stats": stats,
             "findings": [{**asdict(f), "waived": waiver_key(f) in waivers} for f in findings]},
            indent=2))
    else:
        print(
            f"ipc-contract-audit — {stats['rust_files']} Rust file(s) swept "
            f"({stats['rust_required_present']} of {stats['rust_required']} required present) and "
            f"{stats['ts_files']} of {stats['ts_files_expected']} TS source(s); "
            f"{stats['commands']} command(s), {stats['registered']} registered, "
            f"{stats['invocations']} call site(s) ({stats['typed_invocations']} declare a reply "
            f"type); {stats['shape_compared']} of {stats['typed_invocations']} replies compared, "
            f"{stats['shape_fields']} of those field-by-field"
        )
        for f in findings:
            tag = "ERROR " if f.severity == "error" else "UNRESL"
            mark = "  (waived)" if waiver_key(f) in waivers else ""
            print(f"  {tag} [{f.check:<12}] {f.where}{mark}\n           {f.message}")

    blocking = [f for f in findings if waiver_key(f) not in waivers]
    stale = [k for k in waivers if k not in {waiver_key(f) for f in findings}]
    if not a.json:
        if waivers and len(blocking) != len(findings):
            print("\n  waived, and therefore not blocking:")
            for k, reason in waivers.items():
                if k not in stale:
                    print(f"    {k} — {reason}")
        for k in stale:
            print(f"\n  STALE WAIVER: {k} has no finding. Delete the line; it is now hiding nothing.")
        if not findings and not stale:
            print(f"\n  {SUCCESS}")
        else:
            print(f"\n  {len(blocking)} blocking, {len(findings) - len(blocking)} waived, "
                  f"{len(stale)} stale waiver(s).")
    return 1 if (blocking or stale) else 0


if __name__ == "__main__":
    sys.exit(main())
