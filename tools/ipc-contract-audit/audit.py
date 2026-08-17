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
                a list on both sides, and a fixed-length tuple has the same length on both. A field
                typed `serde_json::Value` is reported by `coverage` rather than counted as agreed
  variants      every Rust serde ENUM that reaches a reply, against the TypeScript string union that
                reads it. This is the quietest drift on the boundary and the last of the three to
                get a check: a renamed FIELD reads `undefined` and something renders blank, but a
                renamed VARIANT arrives as an ordinary, non-null, correctly-typed string that simply
                never equals what the UI compares it to. Nothing throws, nothing is undefined, and
                every branch keyed on it is false forever. Both directions are reported — a variant
                the union does not list (the value arrives with no case for it) and a member the
                enum never sends (a branch that can never fire, which is how a half-finished rename
                survives review). Only a **unit-only** enum is a string on the wire; one that carries
                data is externally tagged and is reported as a disagreement, not compared as a string
  nested        the same three comparisons, applied to every object pair the reply CARRIES, as deep
                as both sides resolve. This check used to not exist, and its absence had a bias
                worth naming: the verdict depended on how deeply a shape happened to be nested
                rather than on whether the two sides agree. Move six fields off a `RuleSummary` and
                into the `RuleData` it carries — one statement instead of seven, a strictly better
                DTO — and six fields stop being audited in the same commit. A gate whose coverage
                shrinks when the code improves is a gate that argues for the worse code. Only pairs
                where BOTH sides resolve to an object are walked; where one resolves and the other
                does not, that is `coverage`'s story, not a second telling of it
  coverage      the audit's own reach — see `_coverage_findings`

Known and deliberate limits, so that "0 blocking" is read for what it is. Each is a place a future
check goes, not a place this one silently claims success:

  * argument STRUCTS — a `#[derive(Deserialize)]` payload handed over inside one key
    (`author_rule({ rule: {..} })`) is compared by its KEY only; the 7 such keys in this tree hide
    34 fields. This is the loudest of the three, which is why it is last: a drifted argument key
    fails to deserialise and the command rejects the call;
  * nullability — `Option<T>` still puts the key on the wire, so a `T | null` read as `T` passes the
    field check. Measured at the top level today: 17 of 17 typed call sites against an
    `Option`-returning command DO declare `| null`, so the surface is currently the 175 `Option`
    FIELDS inside reply DTOs, not the replies themselves;
  * the `variants` check reads DECLARED types. A spec comparing a reply against a string LITERAL
    (`if (n.kind === "blend_1d")`) states the same contract in usage, the way `reads` recovers field
    paths from usage — that is where this check grows next.

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

import jsreads
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


def rust_shape(
    ret: str,
    dtos: dict[str, rustipc.Dto],
    rs: rustipc.RustIpc | None = None,
    prefer_file: str | None = None,
) -> tuple[object, bool, bool]:
    """(shape, is_list, resolved).

    `shape` is a `Dto` for an object, a str for a scalar/opaque kind, or None. `resolved` is False
    when the reader could not decide — never conflated with "it is an empty object", because those
    two produce identical output and only one of them is agreement.

    `rs` + `prefer_file` resolve a bare type name against the declarations of the file that WROTE it
    before falling back to the bare-name table — the same-file rule that makes two unrelated
    `Action`s (a struct in one crate, an enum in another) each resolve to the right one.
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
    if rs is not None and prefer_file is not None:
        d = rustipc.local_dto(t, rs, prefer_file) or rustipc.local_dto(head, rs, prefer_file)
        return (d, is_list, True) if d is not None else (None, is_list, False)
    if t in dtos:
        return dtos[t], is_list, True
    if head in dtos:
        return dtos[head], is_list, True
    return None, is_list, False


def rust_enum(
    ty: str, rs: rustipc.RustIpc, prefer_file: str | None = None
) -> rustipc.SerdeEnum | None:
    """The serde enum behind a field's type, through `Option`/`Vec`/`Box`, or None.

    `prefer_file` is the file that WROTE the type name. A bare `Action` in
    `editor-shell/src/actions.rs` can only mean that file's enum, never the unrelated struct of the
    same name in `core/src/rules.rs` — Rust resolves it through module paths and a bare-name table
    cannot. Without this the reader compared `ActionItem.action` against the wrong type entirely,
    which is the confident-wrong-answer class, not the unresolved one.
    """
    t = ty.strip()
    for _ in range(4):
        head = rustipc.type_head(t)
        if head in ("Option", "Vec", "VecDeque", "Box", "Arc", "Rc") and "<" in t:
            t = t[t.index("<") + 1 : t.rindex(">")].strip()
            continue
        break
    head = rustipc.type_head(t)
    return rustipc.local_enum(t, rs, prefer_file) or rustipc.local_enum(head, rs, prefer_file)


def _variant_findings(
    e: rustipc.SerdeEnum, members: list[str], cmd: str, where: str, path: str, rust_where: str
) -> list[Finding]:
    """Compare the strings a Rust enum SENDS against the strings a TypeScript union ADMITS.

    Two failures, and they are not the same failure:

      * a variant the union does not list — the value arrives and the caller has no case for it. A
        `Record<Union, Label>` lookup returns `undefined`; a `switch` falls through to a default
        that was written for "impossible";
      * a member the enum never sends — a branch that can never fire. Nothing breaks, nothing is
        undefined, and the code reads as though it handles a state the engine stopped having. This
        is how a renamed variant SURVIVES review: the new name is added on both sides and the old
        one is left behind on one of them, where it looks like ordinary defensive coding.

    Both are reported, both as errors, because both are the same fact — the two sides disagree about
    the vocabulary — and downgrading either one would make a renamed variant half-visible.
    """
    out: list[Finding] = []
    sends, admits = list(e.variants), list(members)
    unmodelled = [v for v in sends if v not in admits]
    dead = [m for m in admits if m not in sends]
    if unmodelled:
        out.append(
            Finding(
                "error",
                "variants",
                where,
                f'"{cmd}" — `{path}` is read as {admits}, but `{e.name}` also sends '
                f"{unmodelled} ({rust_where}). The string arrives non-null and correctly typed and "
                "matches no case the caller has; nothing throws",
                f"variants@{cmd}.{e.name}",
            )
        )
    if dead and not e.has_other:
        out.append(
            Finding(
                "error",
                "variants",
                where,
                f'"{cmd}" — `{path}` admits {dead}, which `{e.name}` never sends '
                f"({rust_where} sends {sends}) — {'that comparison is' if len(dead) == 1 else 'those comparisons are'} "
                "false forever",
                f"variants@{cmd}.{e.name}",
            )
        )
    return out


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


#: How far past a reply's own keys the reader will follow it. Two levels of nesting is already past
#: everything this boundary declares; the cap exists so a pathological type graph cannot hang the gate,
#: not because depth 7 would be uninteresting.
_MAX_DEPTH = 6


def _nested_findings(
    r_shape: rustipc.Dto,
    t_shape: tsipc.TsType,
    rs: rustipc.RustIpc,
    ts: tsipc.TsIpc,
    cmd: str,
    where: str,
) -> tuple[list[Finding], int, int, int, set[str]]:
    """Follow a reply PAST its own keys, and compare each nested object pair the same way.

    Comparing only the outermost object was a stated limit, and it was load-bearing in the wrong
    direction: it made the audit's verdict depend on how deeply a shape happened to be nested rather
    than on whether the two sides agree. Move six fields off a reply and into a struct the reply
    carries — a strictly better DTO, one statement instead of seven — and six fields silently stop
    being audited. A gate whose coverage shrinks when the code improves is a gate that argues for the
    worse code.

    Only pairs where **both** sides resolve to an object are walked. Where one side resolves and the
    other does not, that is the coverage story the `Value`-opaque check already tells, and repeating
    it here would be noise rather than reach. Returns the findings and how many nested object pairs
    were compared, so the header can state this reach too instead of implying it.

    A field whose Rust type is a serde ENUM is compared here too, and it is compared before the
    both-sides-resolve gate rather than after: an enum is not in `dtos`, so `rust_shape` calls it
    unresolved and the walk used to stop dead at every one of them. That silence looked exactly like
    agreement. Returns `(findings, object pairs, enum pairs compared, enum pairs unresolvable)`.
    """
    out: list[Finding] = []
    pairs = 0
    enum_pairs = enum_unresolved = 0
    enum_names: set[str] = set()
    seen: set[tuple[str, str]] = {(r_shape.name, t_shape.name)}
    # (rust dto, ts type, path, depth) — breadth-first, so a shallow drift is reported before a deep one.
    queue: list[tuple[rustipc.Dto, tsipc.TsType, str, int]] = [(r_shape, t_shape, t_shape.name, 0)]
    while queue:
        r_dto, t_ty, path, depth = queue.pop(0)
        if depth >= _MAX_DEPTH:
            continue
        carried = {k for k, _ in r_dto.fields}
        for key, _opt in t_ty.fields:
            if key not in carried or key in r_dto.opaque:
                continue  # absence is the top-level check's finding; opaque is already reported
            r_ft = r_dto.field_types.get(key)
            t_ft = t_ty.field_types.get(key)
            if not r_ft or not t_ft:
                continue
            here_e = f"{path}.{key}"
            # The file that wrote the field's type is the file whose declarations it can name.
            e = rust_enum(r_ft, rs, r_dto.file)
            if e is not None:
                members = tsipc.string_union(t_ft, ts.aliases)
                if members is not None and not e.string_like:
                    # The caller reads a string union; the shell does not send a string. This is not
                    # reach loss, it is a disagreement — every one of those comparisons is false and
                    # the value is an object besides.
                    out.append(
                        Finding(
                            "error", "variants", where,
                            f'"{cmd}" — `{here_e}` is read as the string union {members}, but '
                            f"`{e.name}` is not a string on the wire: {e.why_not} "
                            f"({e.file}:{e.line})",
                            f"variants@{cmd}.{e.name}",
                        )
                    )
                elif not e.string_like or members is None:
                    # Named, not skipped. An enum the reader cannot compare and an enum that agrees
                    # must not produce the same silence — that is this tool's founding lesson.
                    enum_unresolved += 1
                else:
                    enum_pairs += 1
                    enum_names.add(e.name)
                    out += _variant_findings(
                        e, members, cmd, where, here_e, f"{e.file}:{e.line}"
                    )
                continue
            r_sub, r_list, r_ok = rust_shape(r_ft, rs.dtos, rs, r_dto.file)
            t_sub, t_list, t_ok = ts_shape(t_ft, ts.types, ts.aliases)
            if not (r_ok and t_ok):
                continue
            here = f"{path}.{key}"
            if r_list != t_list:
                out.append(
                    Finding(
                        "error", "nested", where,
                        f'"{cmd}" — `{here}` is {"a list" if t_list else "a single value"} in TypeScript, '
                        f"but `{r_dto.name}.{key}` is `{r_ft}` ({r_dto.file}:{r_dto.line})",
                        f"nested@{cmd}",
                    )
                )
                continue
            if isinstance(r_sub, rustipc.Dto) and isinstance(t_sub, tsipc.TsType):
                pairs += 1
                sub_carried = {k for k, _ in r_sub.fields}
                if not any(k.startswith("<flatten:") for k in sub_carried):
                    missing = [k for k, o in t_sub.fields if not o and k not in sub_carried]
                    if missing:
                        out.append(
                            Finding(
                                "error", "nested", where,
                                f'"{cmd}" — `{here}` is read as `{t_sub.name}`, which requires {missing}; '
                                f"`{r_sub.name}` never sends {'it' if len(missing) == 1 else 'them'} "
                                f"({r_sub.file}:{r_sub.line} carries {sorted(sub_carried)}). Nothing throws: "
                                "JavaScript reads `undefined` this far down exactly as it does at the top",
                                f"nested@{cmd}",
                            )
                        )
                tag = (r_sub.name, t_sub.name)
                if tag not in seen:
                    seen.add(tag)
                    queue.append((r_sub, t_sub, here, depth + 1))
            elif isinstance(r_sub, str) and isinstance(t_sub, str):
                why = _scalar_conflict(r_sub, t_sub)
                if why:
                    out.append(
                        Finding(
                            "error", "nested", where,
                            f'"{cmd}" — `{here}` {why}, but `{r_dto.name}.{key}` is `{r_ft}` '
                            f"({r_dto.file}:{r_dto.line})",
                            f"nested@{cmd}",
                        )
                    )
    return out, pairs, enum_pairs, enum_unresolved, enum_names


def _shape_findings(
    rs: rustipc.RustIpc, ts: tsipc.TsIpc
) -> tuple[list[Finding], int, int, int, int, int, list[str]]:
    """Returns the findings and, so the header can state the check's REACH, how many replies were
    actually compared and how many of those got a field-by-field comparison rather than only a
    kind/list one. A `shape` line that says nothing is the output of both "they agree" and "I never
    resolved either side", and only the header can tell a reader which."""
    out: list[Finding] = []
    compared = fields_compared = nested_pairs = 0
    enum_pairs = enum_unresolved = 0
    #: WHICH enums were reached, not just how many pairs. 63 pairs across 8 distinct enums and 63
    #: across 29 are very different states of coverage, and only the second number distinguishes
    #: them — the rest sit behind `serde_json::Value` reply fields that `coverage` already waives.
    enum_names: set[str] = set()
    for inv in ts.invocations:
        if inv.cmd is None or inv.targ is None or inv.cmd not in rs.commands:
            continue
        cmd = rs.commands[inv.cmd]
        if cmd.unreadable:
            continue
        where = f"{inv.file}:{inv.line}"
        # A command whose whole reply IS an enum. `rust_shape` cannot resolve it — an enum is not a
        # DTO — so before this check every such call site fell to `coverage` as unresolved.
        top_enum = rust_enum(cmd.ret, rs, cmd.file)
        if top_enum is not None:
            members = tsipc.string_union(inv.targ, ts.aliases)
            if not top_enum.string_like or members is None:
                enum_unresolved += 1
            else:
                enum_pairs += 1
                enum_names.add(top_enum.name)
                out += _variant_findings(
                    top_enum, members, inv.cmd, where,
                    f"invoke<{inv.targ}>", f"{top_enum.file}:{top_enum.line}",
                )
            continue
        r_shape, r_list, r_ok = rust_shape(cmd.ret, rs.dtos)
        t_shape, t_list, t_ok = ts_shape(inv.targ, ts.types, ts.aliases)
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
            continue  # the shapes already disagree here; a walk below it would report the same break twice
        nf, np, ne, nu, nn = _nested_findings(r_shape, t_shape, rs, ts, inv.cmd, where)
        out.extend(nf)
        nested_pairs += np
        enum_pairs += ne
        enum_unresolved += nu
        enum_names |= nn
    return (out, compared, fields_compared, nested_pairs, enum_pairs, enum_unresolved,
            sorted(enum_names))


def _tuple_read_findings(
    rs: rustipc.RustIpc, tuples: list[jsreads.TupleRead]
) -> tuple[list[Finding], int]:
    """`const [a, , b] = await invoke("c")` claims the reply has at least three positions.

    A destructuring pattern is a binding form the field reader does not recognise, and an
    unrecognised binding form is the one kind of reach loss no counter can show: the call site
    increments nothing and looks exactly like a call site that binds nothing. 16 real sites in this
    tree destructure a tuple reply. Arity is part of the contract — this tool already learned that
    on the typed side, where a Rust 5-tuple and a TypeScript 3-tuple compared equal until the length
    was compared. Claiming FEWER positions than the reply has is legal JavaScript and not reported;
    claiming more is `undefined`.
    """
    out: list[Finding] = []
    compared = 0
    for tr in tuples:
        cmd = rs.commands.get(tr.cmd)
        if cmd is None or cmd.unreadable:
            continue
        shape, _, ok = rust_shape(cmd.ret, rs.dtos)
        if not ok or not (isinstance(shape, str) and shape.startswith("<tuple:")):
            continue
        arity = shape[len("<tuple:") : -1]
        if not arity.isdigit():
            continue
        compared += 1
        if tr.positions > int(arity):
            out.append(
                Finding(
                    "error",
                    "reads",
                    f"{tr.file}:{tr.line}",
                    f'invoke("{tr.cmd}") is destructured as `{tr.text}`, claiming '
                    f"{tr.positions} position(s), but the command returns `{cmd.ret}` — "
                    f"{arity} ({cmd.file}:{cmd.line}). The extra name(s) are `undefined`",
                    f"reads@{tr.cmd}.<arity>",
                )
            )
    return out, compared


def _read_findings(
    rs: rustipc.RustIpc, reads: list[jsreads.Read]
) -> tuple[list[Finding], int, int]:
    """Compare what untyped JavaScript READS off a reply against what the reply carries.

    The `shape` check above needs an `invoke<T>` to compare. The 142-file E2E suite never writes one
    — and reads the replies anyway, in plain `.js` that `tsc` never opens and vitest never loads.
    `rules[0].rule.event` is the same assertion as `invoke<RuleSummary[]>`, made in usage instead of
    in a declaration, and until this check existed the only thing that ever evaluated it was a human
    running wdio against a packaged `.exe`.

    That gap is not theoretical: collapsing `RuleSummary` to `{ id, rule }` broke four specs — one
    with a TypeError, one inside M12.1's own live acceptance — with all five gates green, because
    none of them looks at this tree for anything but `invoke()` call sites.

    Conservative by construction, because a false finding here would be read as noise and would cost
    the check its authority: a field is called absent only when the reply resolves to a struct whose
    keys are fully enumerable. A `serde(flatten)` splices in keys this reader cannot name, so a
    struct carrying one can never prove absence; a `Value` has no names at all; anything the walk
    cannot resolve stops the walk and is counted as unresolved rather than reported as agreement.

    Returns the findings, the number of path steps actually compared, and the number of reads whose
    walk stopped early — the reach, stated rather than implied.
    """
    out: list[Finding] = []
    steps_compared = unresolved = 0
    for rd in reads:
        cmd = rs.commands.get(rd.cmd)
        if cmd is None or cmd.unreadable:
            continue  # an unregistered name is the `command` check's finding, not a second telling
        shape, is_list, ok = rust_shape(cmd.ret, rs.dtos)
        if not ok:
            unresolved += 1
            continue
        where = f"{rd.file}:{rd.line}"
        walked: list[str] = []
        for step in rd.path:
            if step == "[]":
                if not is_list:
                    break  # `x["key"]` on a map, a tuple index, a string index — not provably wrong
                is_list = False
                walked.append("[]")
                continue
            if is_list or not isinstance(shape, rustipc.Dto):
                break
            carried = {k for k, _ in shape.fields}
            if any(k.startswith("<flatten:") for k in carried):
                break  # flatten makes absence unprovable, not false
            if step not in carried:
                path = "".join(f"[0]" if s == "[]" else f".{s}" for s in walked)
                out.append(
                    Finding(
                        "error",
                        "reads",
                        where,
                        f'invoke("{rd.cmd}") is read as {rd.via}{path}.{step}, but '
                        f"`{shape.name}` never sends `{step}` ({shape.file}:{shape.line} carries "
                        f"{sorted(carried)}) — this file is plain .js that no type-checker opens, so "
                        "the read is `undefined` at run time and only the wdio run says so",
                        f"reads@{rd.cmd}.{step}",
                    )
                )
                break
            steps_compared += 1
            walked.append(step)
            if step in shape.opaque:
                break  # the key is there; everything inside it is a `Value`, so unchecked
            ty = shape.field_types.get(step)
            if not ty:
                break
            shape, is_list, ok = rust_shape(ty, rs.dtos)
            if not ok:
                break
        else:
            continue
        if len(walked) < len(rd.path) and not any(f.where == where for f in out[-1:]):
            unresolved += 1
    return out, steps_compared, unresolved


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
    # A name that is both a struct and an enum resolves per-file (`local_dto`/`local_enum`), which
    # is exact wherever the field's own file declares it. What stays genuinely ambiguous is a THIRD
    # file naming the type without declaring it — there the bare-name table decides, and it can be
    # wrong. Only that residue is reported, so the finding names a risk rather than a fact the
    # reader has already handled.
    for name, e in sorted(rs.enums.items()):
        if not e.struct_collision:
            continue
        elsewhere = sorted(
            {
                d.file
                for d in rs.dtos.values()
                if any(rustipc.type_head(t.split("<")[-1].strip("<> ")) == name
                       or rustipc.type_head(t) == name
                       for t in d.field_types.values())
                and (d.file, name) not in rs.dtos_local
                and (d.file, name) not in rs.enums_local
            }
        )
        if elsewhere:
            out.append(
                Finding(
                    "unresolved",
                    "coverage",
                    f"{e.file}:{e.line}",
                    f"`{name}` is a serde enum here AND a struct at {e.struct_collision}, and "
                    f"{elsewhere} name(s) the type without declaring either. Same-file resolution "
                    "cannot decide those, so the bare-name table does — and it may pick the wrong "
                    "kind. Disambiguate the name",
                    f"coverage@enum-struct-collision.{name}",
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

    # What the untyped side READS. Recovered per file, from the same text the call-site parser saw,
    # because a read is only attributable to a reply within the block that bound it.
    reads: list[jsreads.Read] = []
    tuple_reads: list[jsreads.TupleRead] = []
    bound = 0
    for rel, text in ts_call_src + e2e_src:
        r = jsreads.parse(tsipc.strip_comments(text), rel)
        reads += r.reads
        tuple_reads += r.tuples
        bound += r.bound

    findings = _coverage_findings(root, missing, rs, ts)
    compared = fields_compared = nested_pairs = 0
    read_steps = read_unresolved = tuples_compared = 0
    enum_pairs = enum_unres = 0
    enum_names: list[str] = []
    if not any(f.check == "coverage" and "audit root does not exist" in f.message for f in findings) \
            and not missing:
        findings += _registration_findings(rs)
        findings += _command_findings(rs, ts)
        findings += _argument_findings(rs, ts)
        (shape, compared, fields_compared, nested_pairs, enum_pairs, enum_unres,
         enum_names) = _shape_findings(rs, ts)
        findings += shape
        rf, read_steps, read_unresolved = _read_findings(rs, reads)
        findings += rf
        tf, tuples_compared = _tuple_read_findings(rs, tuple_reads)
        findings += tf

    stats = {
        "rust_files": len(rust_src),
        "rust_required_present": len(REQUIRED_RUST) - len(rust_missing),
        "rust_required": len(REQUIRED_RUST),
        "ts_required_present": len({"/".join(p) for p in TS_CALL_SOURCES + TS_TYPE_SOURCES})
                               - len([m for m in missing if m.startswith("editor/")]),
        "ts_required": len({"/".join(p) for p in TS_CALL_SOURCES + TS_TYPE_SOURCES}),
        "ts_swept": len(e2e_src),
        "commands": len([c for c in rs.commands.values() if not c.unreadable]),
        "registered": len(rs.registered),
        "invocations": len(ts.invocations),
        "typed_invocations": len([i for i in ts.invocations if i.targ]),
        "shape_compared": compared,
        "shape_fields": fields_compared,
        "nested_pairs": nested_pairs,
        "untyped_bound": bound,
        "read_paths": len(reads),
        "read_steps": read_steps,
        "read_unresolved": read_unresolved,
        "tuple_reads": len(tuple_reads),
        "tuples_compared": tuples_compared,
        "enums": len(rs.enums),
        "enums_string_like": len([e for e in rs.enums.values() if e.string_like]),
        "enum_pairs": enum_pairs,
        "enum_unresolved": enum_unres,
        "enum_names": enum_names,
        "dtos": len(rs.dtos),
        "ts_types": len(ts.types),
    }
    order = {"registration": 0, "command": 1, "arguments": 2, "shape": 3, "nested": 4,
             "variants": 5, "reads": 6, "coverage": 7}
    findings.sort(key=lambda f: (order.get(f.check, 9), f.where, f.message))
    return findings, stats


SUCCESS = (
    "every invoked command exists, every argument key is accepted, every reply field the UI reads "
    "— at the top level and every level under it the reader can resolve — is one the shell sends, "
    "and every enum reaching the UI sends exactly the strings the UI compares against."
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
            f"{stats['ts_required_present']} of {stats['ts_required']} required TS source(s) "
            f"+ {stats['ts_swept']} E2E file(s) swept; "
            f"{stats['commands']} command(s), {stats['registered']} registered, "
            f"{stats['invocations']} call site(s) ({stats['typed_invocations']} declare a reply "
            f"type); {stats['shape_compared']} of {stats['typed_invocations']} replies compared, "
            f"{stats['shape_fields']} of those field-by-field, and {stats['nested_pairs']} nested "
            f"object pair(s) followed below the top level; "
            f"{stats['untyped_bound']} untyped reply(s) bound in JavaScript, "
            f"{stats['read_paths']} distinct field path(s) read off them, {stats['read_steps']} "
            f"step(s) compared ({stats['read_unresolved']} walk(s) stopped early), and "
            f"{stats['tuples_compared']} of {stats['tuple_reads']} positional destructuring(s) "
            f"compared for arity; "
            f"{stats['enums_string_like']} of {stats['enums']} serde enum(s) are a bare string on "
            f"the wire, and {stats['enum_pairs']} enum/string-union pair(s) across "
            f"{len(stats['enum_names'])} distinct enum(s) had their variant sets compared "
            f"({stats['enum_unresolved']} could not be)"
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
