#!/usr/bin/env python3
"""gpu-contract-audit — does the Rust still describe the same pipeline the WGSL does?

Every render pipeline in this engine is described twice: once in WGSL, and once in Rust. wgpu
compares the two at `create_render_pipeline` and refuses the pipeline when they disagree — so a
drift is not a warning, it is a pipeline that cannot exist. The cost of that check is that it only
happens on a machine with a GPU, at run time, in a build that got that far.

`render.rs` survives on the strength of being run constantly. The offscreen examples are not: they
COMPILE, so `cargo clippy --all-targets` reports them healthy, while every contract inside them
rots silently. `mesh_frame_bench` accumulated four independent drifts that way, and the repository's
only automatable render evidence went with it.

This runs the same comparison at rest, with no GPU and no toolchain:

  attributes   every `@location` the vertex entry reads is supplied, with a compatible format
  stages       every `@location` the fragment entry reads is written by the vertex entry, same type
  groups       every `@group` the entry actually reaches is present in the pipeline layout
  structs      a `#[repr(C)]` struct the shader also declares has the same size under both layouts
  entries      the entry point named in Rust exists in the shader

Exit status is 1 if any of those fail, so it can stand as a gate. Anything it could not read is
reported as UNRESOLVED and fails too — an audit that silently skips what it does not understand is
the failure it exists to prevent.

Usage:  python3 audit.py [--root DIR] [--json] [--self-test]
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
from dataclasses import dataclass, asdict

import rustgpu
import wgsl

# Which Rust files describe pipelines, relative to the audit root. Each is paired with the shaders
# it `include_str!`s, so the pairing is read from the source rather than configured here.
# Split into components rather than written with "/", so the glob is the same on Windows — where
# this crate is actually built, and where CI runs it.
#
# **Recursive on purpose.** These were `("src", "*.rs")` — one level, no `**`. `render.rs` is a
# 334 KB file whose obvious repair is to split it into `src/render/`, and the moment it moved there
# the audit stopped seeing the renderer AT ALL: not an error, not a warning, just a smaller number in
# a header line nobody diffs. Losing the renderer does not merely leave it unchecked — it is the
# *reference* the `parity` check measures examples against, so its absence silently switched that
# check off for every example (proved: `progress/gpu-contract-audit/discovery-blind-spot.md`).
# A glob that fails to find the thing it is auditing must never read as "clean", which is what
# `_coverage_findings` below now enforces.
RUST_GLOBS = (
    ("src", "**", "*.rs"),
    ("examples", "**", "*.rs"),
    ("tests", "**", "*.rs"),
    ("benches", "**", "*.rs"),
)

# A pipeline-creating CALL, not a mention. The trailing `(` and leading `.` keep a source-contract
# test that merely asserts on the string `"create_render_pipeline"` from reading as a renderer.
PIPELINE_CALL = re.compile(r"\.create_(?:render|compute)_pipeline\s*\(")


@dataclass
class Finding:
    severity: str  # "error" | "unresolved"
    check: str
    where: str
    message: str


# ── Rust type sizes, under `#[repr(C)]` ───────────────────────────────────────────────────────────

_PRIM = {
    "f32": (4, 4),
    "u32": (4, 4),
    "i32": (4, 4),
    "f64": (8, 8),
    "u64": (8, 8),
    "i64": (8, 8),
    "u16": (2, 2),
    "i16": (2, 2),
    "u8": (1, 1),
    "i8": (1, 1),
    "bool": (1, 1),
}
_ARRAY = re.compile(r"^\[(.+);\s*(\d+)\s*\]$")


def rust_layout(ty: str, siblings: dict[str, rustgpu.ReprCStruct] | None = None) -> tuple[int, int] | None:
    """(align, size) for the subset of types a GPU-shared `#[repr(C)]` struct may contain.

    `siblings` lets a nested `#[repr(C)]` field resolve — `Camera` holds a `ColourUniform`, and
    refusing to size that would mean never checking the one struct most likely to drift.
    """
    ty = ty.strip().rstrip(",")
    if ty in _PRIM:
        return _PRIM[ty]
    m = _ARRAY.match(ty)
    if m:
        inner = rust_layout(m.group(1), siblings)
        if inner is None:
            return None
        a, s = inner
        return a, s * int(m.group(2))
    if siblings and ty in siblings:
        return rust_struct_size(siblings[ty].fields, siblings)
    return None


def rust_struct_size(
    fields: list[tuple[str, str]], siblings: dict[str, rustgpu.ReprCStruct] | None = None
) -> tuple[int, int] | None:
    off, align = 0, 1
    for _, ty in fields:
        got = rust_layout(ty, siblings)
        if got is None:
            return None
        a, s = got
        off = (off + a - 1) // a * a
        off += s
        align = max(align, a)
    return align, (off + align - 1) // align * align


# ── the checks ────────────────────────────────────────────────────────────────────────────────────


def _wgsl_scalar_and_count(ty: str) -> tuple[str, int] | None:
    m = re.fullmatch(r"vec([234])<(\w+)>", ty)
    if m:
        return m.group(2), int(m.group(1))
    if ty in ("f32", "i32", "u32"):
        return ty, 1
    return None


def _layouts_for(expr: str | None, rf: rustgpu.RustFile) -> tuple[list[rustgpu.VertexLayout], list[str]]:
    """Resolve a `buffers:` expression to the vertex layouts it names."""
    if expr is None:
        return [], ["buffers expression missing"]
    e = expr.strip()
    if re.fullmatch(r"&\s*\[\s*\]", e):
        return [], []
    names = [n for n in re.findall(r"[A-Za-z_]\w*", e) if n in rf.vertex_layouts]
    if not names:
        return [], [f"could not resolve buffers expression {e!r} to a VertexBufferLayout"]
    return [rf.vertex_layouts[n] for n in names], []


def check_file(
    rf: rustgpu.RustFile,
    shaders: dict[str, wgsl.Module],
    rel: str,
    reference: dict[str, set[str]] | None = None,
) -> list[Finding]:
    out: list[Finding] = []

    if not rf.pipelines:
        return out
    if not shaders:
        out.append(
            Finding("unresolved", "entries", rel, "declares pipelines but includes no .wgsl source")
        )
        return out

    # One file may include several shaders, and a name can appear in more than one of them —
    # `vs_post` is declared by BOTH post.wgsl and ssao.wgsl. A pipeline is built from ONE module, so
    # the module is the one holding both of its entry points; picking the first match by name
    # instead would audit a pipeline against a shader it was never built from.
    def find_module(vs_name: str | None, fs_name: str | None) -> str | None:
        both = [
            s
            for s, m in shaders.items()
            if (vs_name is None or vs_name in m.entries) and (fs_name is None or fs_name in m.entries)
        ]
        if both:
            return both[0]
        return next((s for s, m in shaders.items() if vs_name in m.entries), None)

    for p in rf.pipelines:
        where = f"{rel}:{p.line} [{p.label}]"
        for u in p.unresolved:
            out.append(Finding("unresolved", "entries", where, f"could not read {u}"))

        # ── entries exist ──────────────────────────────────────────────────────────────────────
        sname = find_module(p.vertex_entry, p.fragment_entry)
        if sname is None:
            if p.vertex_entry:
                out.append(
                    Finding(
                        "error",
                        "entries",
                        where,
                        f"no included shader declares both `{p.vertex_entry}` and `{p.fragment_entry}`",
                    )
                )
            continue
        mod = shaders[sname]
        if p.vertex_entry not in mod.entries:
            out.append(
                Finding("error", "entries", where, f"vertex entry point `{p.vertex_entry}` is not in {sname}")
            )
            continue
        if p.fragment_entry and p.fragment_entry not in mod.entries:
            out.append(
                Finding("error", "entries", where, f"fragment entry point `{p.fragment_entry}` is not in {sname}")
            )
        ventry = mod.entries[p.vertex_entry]
        fs = (sname, mod.entries[p.fragment_entry]) if p.fragment_entry in mod.entries else None

        # ── attributes ─────────────────────────────────────────────────────────────────────────
        required = mod.inputs(ventry.name)
        layouts, problems = _layouts_for(p.buffers, rf)
        for msg in problems:
            out.append(Finding("unresolved", "attributes", where, msg))
        if problems:
            # An unreadable `buffers:` expression means the layouts are UNKNOWN, not empty. Falling
            # through printed a confident "no vertex buffers are bound" for every location the shader
            # reads — a diagnosis invented on top of "I could not read it", pointing the reader at the
            # shader when the repair belongs in rustgpu.py. The unresolved finding above already
            # blocks; adding a wrong reason to a correct failure only costs the next reader an hour.
            # (`buffers: &[]` is a different case — genuinely empty, no problem reported, still checked.)
            continue
        provided: dict[int, rustgpu.Attribute] = {}
        for lay in layouts:
            for a in lay.attributes:
                provided[a.location] = a
        for loc in sorted(required):
            want = required[loc]
            got = provided.get(loc)
            if got is None:
                out.append(
                    Finding(
                        "error",
                        "attributes",
                        where,
                        f"`{ventry.name}` reads @location({loc}) : {want}, and no vertex buffer layout supplies it"
                        + (f" (layouts supply {sorted(provided)})" if provided else " (no vertex buffers are bound)"),
                    )
                )
                continue
            info = rustgpu.format_info(got.format)
            if info is None:
                out.append(Finding("unresolved", "attributes", where, f"unknown vertex format {got.format!r}"))
                continue
            scalar, count, size = info
            wanted = _wgsl_scalar_and_count(want)
            if wanted is None:
                out.append(Finding("unresolved", "attributes", where, f"unreadable shader input type {want!r}"))
                continue
            wscalar, wcount = wanted
            if scalar != wscalar:
                out.append(
                    Finding(
                        "error",
                        "attributes",
                        where,
                        f"@location({loc}) is {got.format} ({scalar}) but `{ventry.name}` reads {want} ({wscalar})",
                    )
                )
            elif count < wcount:
                out.append(
                    Finding(
                        "error",
                        "attributes",
                        where,
                        f"@location({loc}) supplies {count} component(s) but `{ventry.name}` reads {want}",
                    )
                )
        # locations supplied but not read are legal in wgpu, and are not reported.

        # ── stages ─────────────────────────────────────────────────────────────────────────────
        if fs is not None:
            _, fentry = fs
            vout = mod.outputs(ventry.name)
            fin = mod.inputs(fentry.name)
            for loc in sorted(fin):
                if loc not in vout:
                    out.append(
                        Finding(
                            "error",
                            "stages",
                            where,
                            f"`{fentry.name}` reads @location({loc}) : {fin[loc]}, which `{ventry.name}` does not write "
                            f"(it writes {sorted(vout)})",
                        )
                    )
                elif vout[loc] != fin[loc]:
                    out.append(
                        Finding(
                            "error",
                            "stages",
                            where,
                            f"@location({loc}) is {vout[loc]} out of `{ventry.name}` but {fin[loc]} into `{fentry.name}`",
                        )
                    )

        # ── parity ─────────────────────────────────────────────────────────────────────────────
        # An offscreen example exists to reproduce what the app draws. wgpu is happy to pair a
        # vertex entry with any fragment entry whose inputs it happens to satisfy — `vs_mesh` feeds
        # `fs_main` perfectly well, because `fs_main` reads only @location(0). The pipeline builds,
        # the frame renders, and the evidence is of a shader the application never runs: flat
        # unlit colour where the app evaluates a BRDF. Nothing else in the audit can see that, so
        # the pairing the renderer uses is the reference.
        # A vertex entry the reference has never heard of used to fall out of this `if` in silence,
        # which is the partial-reference hole: lose SOME of the renderer and the examples drawing the
        # lost entries go unmeasured, with the same "0 findings" as agreement. There are only two
        # readings and both are defects — either the renderer genuinely never draws this entry (so
        # the frame is not evidence of the application) or the audit failed to read the pipeline that
        # does. Say so rather than skip.
        if reference is not None and p.vertex_entry and p.vertex_entry not in reference:
            out.append(
                Finding(
                    "error",
                    "parity",
                    where,
                    f"draws `{p.vertex_entry}`, but no pipeline audited under src/ draws it — either the "
                    f"application never renders this entry, or the renderer's pipeline for it was not read. "
                    f"src/ draws: {', '.join(sorted('`%s`' % e for e in reference)) or '(nothing)'}",
                )
            )
        if reference is not None and p.vertex_entry in reference:
            expected_fs, expected_topo = reference[p.vertex_entry]
            if p.fragment_entry and expected_fs and p.fragment_entry not in expected_fs:
                out.append(
                    Finding(
                        "error",
                        "parity",
                        where,
                        f"pairs `{p.vertex_entry}` with `{p.fragment_entry}`, but the renderer draws it with "
                        f"{' or '.join(sorted('`%s`' % e for e in expected_fs))} — this frame would not be "
                        f"evidence of what the application renders",
                    )
                )
            # Topology is the other half of "what is actually drawn", and it has no wgpu error either:
            # `vs_grid` computes `corners[vi]` over a 6-vertex quad, so drawing it as a LineList over 164
            # vertices is a valid pipeline that clamps 158 of them onto one corner. The shader moved from
            # hardware lines to a coverage-shaded quad; a draw that did not follow renders a different
            # picture, silently.
            if p.topology and expected_topo and p.topology not in expected_topo:
                out.append(
                    Finding(
                        "error",
                        "parity",
                        where,
                        f"draws `{p.vertex_entry}` as {p.topology}, but the renderer draws it as "
                        f"{' or '.join(sorted(expected_topo))} — the same vertex entry, a different primitive",
                    )
                )

        # ── groups ─────────────────────────────────────────────────────────────────────────────
        used = dict(mod.resources_used(ventry.name))
        if fs is not None:
            used.update(mod.resources_used(fs[1].name))
        needed: dict[int, list[str]] = {}
        for g in used.values():
            needed.setdefault(g.group, []).append(f"@binding({g.binding}) {g.name}")
        if p.layout is None:
            if needed:
                out.append(Finding("unresolved", "groups", where, "pipeline layout expression is unreadable"))
        elif p.layout not in rf.pipeline_layouts:
            out.append(
                Finding("unresolved", "groups", where, f"pipeline layout `{p.layout}` is not declared in this file")
            )
        else:
            have = len(rf.pipeline_layouts[p.layout].groups)
            for gi in sorted(needed):
                if gi >= have:
                    out.append(
                        Finding(
                            "error",
                            "groups",
                            where,
                            f"the shader reaches @group({gi}) — {', '.join(sorted(needed[gi]))} — but `{p.layout}` "
                            f"declares only {have} bind group layout(s)",
                        )
                    )

    # ── structs ────────────────────────────────────────────────────────────────────────────────
    # Two different rules, because the two address spaces are not the same promise:
    #
    #   uniform  a shader may declare a PREFIX of what the host writes, and this codebase relies on
    #            that deliberately — `ssao.wgsl` names no colour block, which is what stops it from
    #            being able to restate one wrongly. So: shader ≤ host. Declaring MORE is what wgpu
    #            rejects at draw, with a message that names two byte counts and nothing else.
    #   storage  an `array<T>` is indexed by stride. A host element of a different size does not
    #            fail; it silently reads the next element's bytes as this one's fields, which is a
    #            wrong picture rather than an error. So: exactly equal.
    for name, rs in rf.structs.items():
        for sname, mod in shaders.items():
            if name not in mod.structs:
                continue
            ws = mod.structs[name]
            if ws.size == 0:
                continue  # an IO envelope, not a host-shared block
            spaces = {g.address_space for g in mod.globals.values() if _names_type(g.ty, name)}
            got = rust_struct_size(rs.fields, rf.structs)
            if got is None:
                out.append(
                    Finding(
                        "unresolved",
                        "structs",
                        f"{rel}:{rs.line} [{name}]",
                        f"contains a field type this audit cannot size: "
                        f"{[t for _, t in rs.fields if rust_layout(t, rf.structs) is None]}",
                    )
                )
                continue
            _, size = got
            if "storage" in spaces and size != ws.size:
                out.append(
                    Finding(
                        "error",
                        "structs",
                        f"{rel}:{rs.line} [{name}]",
                        f"Rust lays it out as {size} bytes; {sname} declares {ws.size} and indexes it as a "
                        f"storage array, so element N reads the wrong bytes "
                        f"(shader fields at {[f.offset for f in ws.fields]})",
                    )
                )
            elif "storage" not in spaces and ws.size > size:
                out.append(
                    Finding(
                        "error",
                        "structs",
                        f"{rel}:{rs.line} [{name}]",
                        f"{sname} declares {ws.size} bytes but Rust writes only {size}; a uniform may be a "
                        f"prefix of the buffer, never longer than it",
                    )
                )
    return out


def _names_type(ty: str, name: str) -> bool:
    return re.search(rf"\b{re.escape(name)}\b", ty) is not None


# ── driver ────────────────────────────────────────────────────────────────────────────────────────


def _rust_files(root: str) -> list[str]:
    import glob

    out: list[str] = []
    for parts in RUST_GLOBS:
        for p in glob.glob(os.path.join(root, *parts), recursive=True):
            # `target/` holds generated copies of the very sources being audited; auditing a build
            # artefact would double every finding and pin them to paths nobody edits.
            if f"{os.sep}target{os.sep}" in p:
                continue
            out.append(p)
    return sorted(set(out))


def _coverage_findings(
    root: str,
    scanned: list[tuple[str, str]],
    parsed: list[tuple[str, rustgpu.RustFile, dict[str, wgsl.Module]]],
    reference: dict[str, tuple[set[str], set[str]]],
) -> list[Finding]:
    """Findings about the audit's own reach — the ones that keep "clean" from meaning "invisible".

    Every other check in this file compares two statements of a contract. These compare the audit
    against the tree, because a checker that silently stops looking is worse than no checker: it
    reports the same "0 findings" it reports when everything is right. Pointed at a directory that
    does not exist, this tool used to print *"every pipeline's Rust description matches the WGSL it
    names"* and exit 0 — its success message, for a tree it never opened.
    """
    out: list[Finding] = []
    read: set[str] = {rel for rel, _, _ in parsed}

    # 0. The tree itself. A renamed crate directory, a wrong --root, or a CI job whose working
    #    directory moved must never read as agreement. This one is checked first because every
    #    finding below it would be vacuously absent.
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
    if not scanned:
        return [
            Finding(
                "error",
                "coverage",
                root.replace(os.sep, "/"),
                f"no .rs file was found under {'/, '.join(p[0] for p in RUST_GLOBS)}/ — the audit root "
                "exists but holds none of the trees this gate reads",
            )
        ]
    if not any(rf.pipelines for _, rf, _ in parsed):
        out.append(
            Finding(
                "error",
                "coverage",
                root.replace(os.sep, "/"),
                f"scanned {len(scanned)} file(s) and found no pipeline anywhere — a renderer the gate "
                "cannot find is not a renderer the gate agrees with",
            )
        )

    # 1. A file that CALLS `create_render_pipeline` and yields no pipeline is unreadable, not clean.
    #    (`//` comments stripped first, so the explanation of a defect never reads as the defect.)
    for rel, raw in scanned:
        if rel in read:
            continue
        if PIPELINE_CALL.search(re.sub(r"//[^\n]*", "", raw)):
            out.append(
                Finding(
                    "unresolved",
                    "coverage",
                    rel,
                    "calls create_render_pipeline, but the audit parsed 0 pipelines from it — this file "
                    "is UNREADABLE to the gate, not clean. Teach rustgpu.py the shape it uses; do not "
                    "leave it looking green",
                )
            )

    # 2. The reference is what `parity` measures examples against. If it is empty, that check has no
    #    ground truth and reports nothing — which is indistinguishable from agreement.
    examples = [rel for rel, rf, _ in parsed if rel.startswith("examples/") and rf.pipelines]
    if examples and not reference:
        out.append(
            Finding(
                "error",
                "coverage",
                "src/",
                "no pipeline was audited under src/, so the parity check has NO reference: the shader "
                f"pairing and primitive of {len(examples)} example file(s) would pass unexamined. The "
                "renderer is not where the audit is looking — check RUST_GLOBS against where it lives now",
            )
        )
    return out


def run(root: str) -> tuple[list[Finding], list[str], list[str]]:
    findings: list[Finding] = []
    audited: list[str] = []
    cache: dict[str, wgsl.Module] = {}
    parsed: list[tuple[str, rustgpu.RustFile, dict[str, wgsl.Module]]] = []
    scanned: list[tuple[str, str]] = []

    for path in _rust_files(root):
        rel = os.path.relpath(path, root).replace(os.sep, "/")
        raw = open(path, encoding="utf-8").read()
        scanned.append((rel, raw))
        rf = rustgpu.parse_rust(rel, raw)
        if not rf.pipelines and not rf.structs:
            continue
        shaders: dict[str, wgsl.Module] = {}
        for inc in rf.shader_sources:
            spath = os.path.normpath(os.path.join(os.path.dirname(path), inc))
            key = os.path.basename(spath)
            if spath not in cache:
                try:
                    cache[spath] = wgsl.parse(open(spath, encoding="utf-8").read())
                except (OSError, wgsl.WgslError) as e:
                    findings.append(Finding("unresolved", "entries", rel, f"cannot read {inc}: {e}"))
                    continue
            shaders[key] = cache[spath]
        if not rf.pipelines and not shaders:
            continue
        parsed.append((rel, rf, shaders))

    # The renderer's own pairings are the reference an example is measured against.
    reference: dict[str, tuple[set[str], set[str]]] = {}
    for rel, rf, _ in parsed:
        if not rel.startswith("src/"):
            continue
        for p in rf.pipelines:
            if not p.vertex_entry:
                continue
            fs, topo = reference.setdefault(p.vertex_entry, (set(), set()))
            if p.fragment_entry:
                fs.add(p.fragment_entry)
            if p.topology:
                topo.add(p.topology)

    for rel, rf, shaders in parsed:
        audited.append(rel)
        findings.extend(check_file(rf, shaders, rel, reference if rel.startswith("examples/") else None))

    findings.extend(_coverage_findings(root, scanned, parsed, reference))

    seen: set[tuple[str, str, str, str]] = set()
    unique: list[Finding] = []
    for f in findings:
        key = (f.severity, f.check, f.where, f.message)
        if key not in seen:
            seen.add(key)
            unique.append(f)
    return unique, audited, [rel for rel, _ in scanned]


def read_waivers(root: str) -> dict[str, str]:
    """`waivers.txt` — `path: reason`, one per line.

    A waiver is not a skip. A waived file is still audited and its findings are still printed in
    full; what changes is that they do not fail the gate. And a waiver whose file has become clean
    FAILS — so it cannot outlive the defect it was written for, which is the way an exception list
    normally turns into a permanent blind spot.
    """
    path = os.path.join(os.path.dirname(os.path.abspath(__file__)), "waivers.txt")
    out: dict[str, str] = {}
    if not os.path.exists(path):
        return out
    for line in open(path, encoding="utf-8"):
        line = line.split("#", 1)[0].strip()
        if not line:
            continue
        if ":" not in line:
            raise SystemExit(f"waivers.txt: `{line}` is not `path: reason`")
        f, reason = line.split(":", 1)
        out[f.strip()] = reason.strip()
    return out


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    here = os.path.dirname(os.path.abspath(__file__))
    ap.add_argument(
        "--root",
        default=os.path.normpath(os.path.join(here, "..", "..", "editor-shell", "src-tauri")),
        help="the crate root holding src/ and examples/",
    )
    ap.add_argument("--json", action="store_true")
    ap.add_argument("--self-test", action="store_true", help="check the audit against known-historic drifts")
    ap.add_argument("--no-waivers", action="store_true", help="fail on every finding, waived or not")
    args = ap.parse_args()

    if args.self_test:
        return self_test()

    findings, audited, scanned = run(args.root)
    waivers = {} if args.no_waivers else read_waivers(args.root)

    if args.json:
        print(
            json.dumps(
                {
                    "audited": audited,
                    "scanned": scanned,
                    "waived": waivers,
                    "findings": [asdict(f) for f in findings],
                },
                indent=2,
            )
        )
        blocking = [f for f in findings if f.where.split(":")[0] not in waivers]
        return 1 if blocking else 0

    # Both numbers, always. "3 file(s)" alone cannot distinguish a tree with three pipeline files
    # from a tree whose renderer the globs stopped matching — and that difference was invisible for
    # exactly as long as only one number was printed.
    print(f"gpu-contract-audit — {len(audited)} of {len(scanned)} scanned file(s): {', '.join(audited)}")
    blocking: list[Finding] = []
    for f in findings:
        mark = "ERROR " if f.severity == "error" else "UNREAD"
        file = f.where.split(":")[0]
        tag = "  (waived)" if file in waivers else ""
        print(f"  {mark} [{f.check:<10}] {f.where}{tag}\n           {f.message}")
        if file not in waivers:
            blocking.append(f)
    if not findings:
        print("  every pipeline's Rust description matches the WGSL it names.")

    stale = [w for w in waivers if not any(f.where.startswith(w) for f in findings)]
    for w in stale:
        print(f"  ERROR  [waiver    ] {w}\n           is waived but audits clean — delete the waiver: {waivers[w]}")

    if waivers:
        print("\n  waived, and therefore not blocking:")
        for f, reason in waivers.items():
            print(f"    {f} — {reason}")

    print(f"\n  {len(blocking)} blocking, {len(findings) - len(blocking)} waived, {len(stale)} stale waiver(s).")
    return 1 if (blocking or stale) else 0


# ── self-test ─────────────────────────────────────────────────────────────────────────────────────
# The audit is only worth running if it fails on the drifts that actually happened. Each case below
# is one of them, reduced to the smallest pair of files that reproduces it.

_SHADER = """
struct Camera { view_proj: mat4x4<f32>, focus: vec4<f32> };
@group(0) @binding(0) var<uniform> cam: Camera;
struct Instance { center: vec3<f32>, scale: f32, rotation: vec4<f32> };
@group(1) @binding(0) var<storage, read> instances: array<Instance>;
struct Light { dir: vec4<f32> };
@group(2) @binding(0) var<storage, read> lights: array<Light>;
struct MeshIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(3) metallic: f32,
};
struct MeshOut { @builtin(position) pos: vec4<f32>, @location(0) c: vec3<f32>, @location(3) mr: vec2<f32> };
struct VsOut { @builtin(position) pos: vec4<f32>, @location(0) color: vec3<f32> };
struct GridOut { @builtin(position) pos: vec4<f32>, @location(0) world_xz: vec2<f32> };
@vertex
fn vs_mesh(v: MeshIn) -> MeshOut {
    var o: MeshOut; o.pos = cam.view_proj * vec4<f32>(v.position, 1.0) * instances[0].scale; return o;
}
@vertex
fn vs_grid(@builtin(vertex_index) vi: u32) -> GridOut {
    var o: GridOut; o.pos = cam.view_proj * vec4<f32>(f32(vi), 0.0, 0.0, 1.0); return o;
}
@fragment
fn fs_mesh(in: MeshOut) -> @location(0) vec4<f32> { return vec4<f32>(in.c, 1.0) * lights[0].dir; }
@fragment
fn fs_grid(in: GridOut) -> @location(0) vec4<f32> { return vec4<f32>(in.world_xz, 0.0, 1.0); }
@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> { return vec4<f32>(in.color, 1.0); }
"""

_CASES: list[tuple[str, str, str]] = [
    (
        "a vertex attribute the shader grew and the host never supplied",
        """
        const S: &str = include_str!("s.wgsl");
        fn build(device: &D) {
            let vbl = wgpu::VertexBufferLayout { array_stride: 24, step_mode: m, attributes: &[
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 0, shader_location: 0 },
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 12, shader_location: 1 },
            ]};
            let lay = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: None, bind_group_layouts: &[Some(&a), Some(&b), Some(&c)], immediate_size: 0 });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor { label: Some("m"),
                layout: Some(&lay),
                vertex: wgpu::VertexState { module: &s, entry_point: Some("vs_mesh"),
                    buffers: std::slice::from_ref(&vbl), compilation_options: d },
                fragment: Some(wgpu::FragmentState { module: &s, entry_point: Some("fs_mesh"),
                    targets: &[], compilation_options: d }), });
        }
        """,
        "attributes",
    ),
    (
        "a fragment entry paired with a vertex entry whose varyings moved on",
        # The real one: the grid grew its own varyings (a world position, not a colour) and kept
        # being paired with `fs_main`. wgpu: "Location[0] ... is not compatible with the provided
        # Float32x2".
        """
        const S: &str = include_str!("s.wgsl");
        fn build(device: &D) {
            let lay = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: None, bind_group_layouts: &[Some(&a)], immediate_size: 0 });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor { label: Some("grid"),
                layout: Some(&lay),
                vertex: wgpu::VertexState { module: &s, entry_point: Some("vs_grid"),
                    buffers: &[], compilation_options: d },
                fragment: Some(wgpu::FragmentState { module: &s, entry_point: Some("fs_main"),
                    targets: &[], compilation_options: d }), });
        }
        """,
        "stages",
    ),
    (
        "a bind group the shader reaches and the pipeline layout never declared",
        """
        const S: &str = include_str!("s.wgsl");
        fn build(device: &D) {
            let vbl = wgpu::VertexBufferLayout { array_stride: 40, step_mode: m, attributes: &[
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 0, shader_location: 0 },
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 12, shader_location: 1 },
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32, offset: 24, shader_location: 3 },
            ]};
            let lay = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: None, bind_group_layouts: &[Some(&a), Some(&b)], immediate_size: 0 });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor { label: Some("m"),
                layout: Some(&lay),
                vertex: wgpu::VertexState { module: &s, entry_point: Some("vs_mesh"),
                    buffers: std::slice::from_ref(&vbl), compilation_options: d },
                fragment: Some(wgpu::FragmentState { module: &s, entry_point: Some("fs_mesh"),
                    targets: &[], compilation_options: d }), });
        }
        """,
        "groups",
    ),
    (
        "a host struct that stopped matching the block the shader indexes",
        """
        const S: &str = include_str!("s.wgsl");
        #[repr(C)]
        #[derive(Clone, Copy)]
        struct Instance { center: [f32; 3], scale: f32 }
        fn build(device: &D) {
            let vbl = wgpu::VertexBufferLayout { array_stride: 40, step_mode: m, attributes: &[
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 0, shader_location: 0 },
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 12, shader_location: 1 },
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32, offset: 24, shader_location: 3 },
            ]};
            let lay = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: None, bind_group_layouts: &[Some(&a), Some(&b), Some(&c)], immediate_size: 0 });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor { label: Some("m"),
                layout: Some(&lay),
                vertex: wgpu::VertexState { module: &s, entry_point: Some("vs_mesh"),
                    buffers: std::slice::from_ref(&vbl), compilation_options: d },
                fragment: Some(wgpu::FragmentState { module: &s, entry_point: Some("fs_mesh"),
                    targets: &[], compilation_options: d }), });
        }
        """,
        "structs",
    ),
]

_CLEAN = """
const S: &str = include_str!("s.wgsl");
#[repr(C)]
#[derive(Clone, Copy)]
struct Instance { center: [f32; 3], scale: f32, rotation: [f32; 4] }
fn build(device: &D) {
    let vbl = wgpu::VertexBufferLayout { array_stride: 40, step_mode: m, attributes: &[
        wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 0, shader_location: 0 },
        wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 12, shader_location: 1 },
        wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32, offset: 24, shader_location: 3 },
    ]};
    let lay = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None, bind_group_layouts: &[Some(&a), Some(&b), Some(&c)], immediate_size: 0 });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor { label: Some("m"),
        layout: Some(&lay),
        vertex: wgpu::VertexState { module: &s, entry_point: Some("vs_mesh"),
            buffers: std::slice::from_ref(&vbl), compilation_options: d },
        fragment: Some(wgpu::FragmentState { module: &s, entry_point: Some("fs_mesh"),
            targets: &[], compilation_options: d }), });
}
"""


# The fifth drift has no wgpu error at all: `vs_mesh` feeds `fs_main` perfectly well, because
# `fs_main` reads only @location(0). It builds, it draws, and it draws the wrong shader. Only the
# renderer's own pairing can call it — so this case needs a `src/` reference to be measured against.
_PARITY_REFERENCE = """
const S: &str = include_str!("s.wgsl");
fn build(device: &D) {
    let vbl = wgpu::VertexBufferLayout { array_stride: 40, step_mode: m, attributes: &[
        wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 0, shader_location: 0 },
        wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 12, shader_location: 1 },
        wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32, offset: 24, shader_location: 3 },
    ]};
    let lay = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None, bind_group_layouts: &[Some(&a), Some(&b), Some(&c)], immediate_size: 0 });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor { label: Some("mesh"),
        layout: Some(&lay),
        vertex: wgpu::VertexState { module: &s, entry_point: Some("vs_mesh"),
            buffers: std::slice::from_ref(&vbl), compilation_options: d },
        fragment: Some(wgpu::FragmentState { module: &s, entry_point: Some("fs_mesh"),
            targets: &[], compilation_options: d }), });
}
"""

_PARITY_EXAMPLE = """
const S: &str = include_str!("../src/s.wgsl");
fn build(device: &D) {
    let vbl = wgpu::VertexBufferLayout { array_stride: 40, step_mode: m, attributes: &[
        wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 0, shader_location: 0 },
        wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 12, shader_location: 1 },
        wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32, offset: 24, shader_location: 3 },
    ]};
    let lay = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None, bind_group_layouts: &[Some(&a), Some(&b), Some(&c)], immediate_size: 0 });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor { label: Some("mesh"),
        layout: Some(&lay),
        vertex: wgpu::VertexState { module: &s, entry_point: Some("vs_mesh"),
            buffers: std::slice::from_ref(&vbl), compilation_options: d },
        fragment: Some(wgpu::FragmentState { module: &s, entry_point: Some("fs_main"),
            targets: &[], compilation_options: d }), });
}
"""

# The sixth drift, and the second with no wgpu error: `vs_grid` computes `corners[vi]` over a
# 6-vertex quad. Drawing it as a LineList is a perfectly valid pipeline that clamps every index past
# 5 onto one corner. The shader moved from hardware lines to a coverage-shaded quad; a draw that did
# not follow renders a different picture and says nothing about it.
_TOPOLOGY_REFERENCE = """
const S: &str = include_str!("s.wgsl");
fn build(device: &D) {
    let lay = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None, bind_group_layouts: &[Some(&a)], immediate_size: 0 });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor { label: Some("grid"),
        layout: Some(&lay),
        vertex: wgpu::VertexState { module: &s, entry_point: Some("vs_grid"),
            buffers: &[], compilation_options: d },
        fragment: Some(wgpu::FragmentState { module: &s, entry_point: Some("fs_grid"),
            targets: &[], compilation_options: d }),
        primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList,
            cull_mode: None, ..Default::default() }, });
}
"""

_TOPOLOGY_EXAMPLE = """
const S: &str = include_str!("../src/s.wgsl");
fn build(device: &D) {
    let lay = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None, bind_group_layouts: &[Some(&a)], immediate_size: 0 });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor { label: Some("grid"),
        layout: Some(&lay),
        vertex: wgpu::VertexState { module: &s, entry_point: Some("vs_grid"),
            buffers: &[], compilation_options: d },
        fragment: Some(wgpu::FragmentState { module: &s, entry_point: Some("fs_grid"),
            targets: &[], compilation_options: d }),
        primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::LineList,
            cull_mode: None, ..Default::default() }, });
}
"""


def _self_test_coverage() -> bool:
    """The audit's reach, checked against itself.

    The five cases above are drifts between two statements of a contract. These four are a different
    species: the gate reporting agreement about code it never read. They are here because all four
    were live in a tool that passed every one of those five.
    """
    import shutil
    import tempfile

    ok = True

    def case(name: str, build, want_check: str, want_text: str, want_severity: str = "error") -> None:
        """`want_text` is not decoration. Written without it, two of these cases passed under a
        mutation that removed the very fix they exist to pin — each caught by a *different* guard
        that happened to fire on the same input. A case that passes for the wrong reason is a case
        that will not notice the regression it was written for."""
        nonlocal ok
        with tempfile.TemporaryDirectory() as tmp:
            root = build(tmp)
            found, _, _ = run(root)
            hit = [
                f
                for f in found
                if f.check == want_check and f.severity == want_severity and want_text in f.message
            ]
            if hit:
                print(f"pass  caught: {name}\n        → {hit[0].message}")
            else:
                ok = False
                print(
                    f"FAIL  missed: {name} (expected a `{want_check}` saying {want_text!r}, "
                    f"got {[(f.check, f.severity, f.message[:60]) for f in found]})"
                )

    def _pair(tmp: str, renderer_dir: str) -> str:
        """A renderer and an example that pairs `vs_mesh` with the wrong fragment entry."""
        rdir = os.path.join(tmp, *renderer_dir.split("/"))
        examples = os.path.join(tmp, "examples")
        os.makedirs(rdir, exist_ok=True)
        os.makedirs(examples, exist_ok=True)
        open(os.path.join(rdir, "s.wgsl"), "w", encoding="utf-8").write(_SHADER)
        open(os.path.join(rdir, "case.rs"), "w", encoding="utf-8").write(_PARITY_REFERENCE)
        rel = "../" * (len(renderer_dir.split("/")) - 1)
        open(os.path.join(examples, "case.rs"), "w", encoding="utf-8").write(
            _PARITY_EXAMPLE.replace('include_str!("../src/s.wgsl")', f'include_str!("../{renderer_dir}/s.wgsl")')
        )
        _ = rel
        return tmp

    # 1. The one that would have fired the day prompt 118's own extraction landed: `render.rs` is a
    #    334 KB file, the obvious repair splits it into `src/render/`, and a one-level glob stops
    #    seeing it. Losing the renderer disables `parity` for every example, silently.
    # Pinned to `fs_mesh` — the reference's OWN fragment entry. That name can only appear if the
    # renderer under src/render/ was actually read; the weaker "any parity error" version of this
    # case passed even with the one-level glob restored, via the unknown-entry guard below.
    case(
        "a renderer one directory deeper than the glob reached",
        lambda tmp: _pair(tmp, "src/render"),
        "parity",
        "the renderer draws it with `fs_mesh`",
    )

    # 2. The renderer gone entirely: parity has no ground truth, and "nothing to compare against"
    #    must not print the same 0 findings as "everything agrees".
    def _no_renderer(tmp: str) -> str:
        examples = os.path.join(tmp, "examples")
        src = os.path.join(tmp, "src")
        os.makedirs(examples)
        os.makedirs(src)
        open(os.path.join(src, "s.wgsl"), "w", encoding="utf-8").write(_SHADER)
        open(os.path.join(examples, "case.rs"), "w", encoding="utf-8").write(_PARITY_EXAMPLE)
        return tmp

    case(
        "an example measured against a renderer that is not there",
        _no_renderer,
        "coverage",
        "the parity check has NO reference",
    )

    # 3. A file that calls create_render_pipeline in a shape the parser cannot read. The prompt's
    #    rule: do not let a file audit clean because it was unreadable.
    def _unreadable(tmp: str) -> str:
        src = os.path.join(tmp, "src")
        os.makedirs(src)
        open(os.path.join(src, "s.wgsl"), "w", encoding="utf-8").write(_SHADER)
        open(os.path.join(src, "case.rs"), "w", encoding="utf-8").write(_PARITY_REFERENCE)
        open(os.path.join(src, "exotic.rs"), "w", encoding="utf-8").write(
            "fn build(d: &D) { let p = make_it!(d, || d.create_render_pipeline(&desc_from_toml())); }"
        )
        return tmp

    case(
        "a pipeline built in a shape the parser cannot read",
        _unreadable,
        "coverage",
        "UNREADABLE to the gate, not clean",
        "unresolved",
    )

    # 4. The partial-reference hole: the renderer IS read, but not the pipeline for the entry this
    #    example draws. `vs_grid` used to fall out of the parity `if` in silence, so an example
    #    drawing something the application never draws reported nothing at all.
    def _orphan_entry(tmp: str) -> str:
        src = os.path.join(tmp, "src")
        examples = os.path.join(tmp, "examples")
        os.makedirs(src)
        os.makedirs(examples)
        open(os.path.join(src, "s.wgsl"), "w", encoding="utf-8").write(_SHADER)
        open(os.path.join(src, "case.rs"), "w", encoding="utf-8").write(_PARITY_REFERENCE)  # draws vs_mesh only
        open(os.path.join(examples, "case.rs"), "w", encoding="utf-8").write(_TOPOLOGY_EXAMPLE)  # draws vs_grid
        return tmp

    case(
        "an example drawing a vertex entry the renderer has no pipeline for",
        _orphan_entry,
        "parity",
        "no pipeline audited under src/ draws it",
    )

    # 5. The root exists and holds Rust, but no pipeline was found in any of it. Distinct from the
    #    case above: this is the shape a renderer moved to another crate leaves behind, and the one
    #    a parser regression leaves behind across the board.
    def _no_pipelines_anywhere(tmp: str) -> str:
        src = os.path.join(tmp, "src")
        os.makedirs(src)
        open(os.path.join(src, "s.wgsl"), "w", encoding="utf-8").write(_SHADER)
        open(os.path.join(src, "case.rs"), "w", encoding="utf-8").write("fn main() { println!(\"no gpu here\"); }\n")
        return tmp

    case(
        "a tree with Rust in it and no pipeline anywhere",
        _no_pipelines_anywhere,
        "coverage",
        "found no pipeline anywhere",
    )

    # 6. The purest form, and it was live: pointed at a directory that does not exist, the tool
    #    printed "every pipeline's Rust description matches the WGSL it names" and exited 0.
    def _missing_root(tmp: str) -> str:
        gone = os.path.join(tmp, "not-a-crate")
        shutil.rmtree(gone, ignore_errors=True)
        return gone

    # Pinned to "does not exist" rather than any coverage error: a missing root also trips the
    # "no .rs file was found" guard, so the weaker version of this case passed with the isdir check
    # removed — reporting a real problem under a misleading name.
    case("an audit root that does not exist", _missing_root, "coverage", "the audit root does not exist")
    return ok


def self_test() -> int:
    import tempfile

    ok = True
    with tempfile.TemporaryDirectory() as tmp:
        src = os.path.join(tmp, "src")
        examples = os.path.join(tmp, "examples")
        os.makedirs(src)
        os.makedirs(examples)
        open(os.path.join(src, "s.wgsl"), "w", encoding="utf-8").write(_SHADER)
        empty = os.path.join(examples, "case.rs")

        def audit(code: str, example: str = "") -> list[Finding]:
            open(os.path.join(src, "case.rs"), "w", encoding="utf-8").write(code)
            open(empty, "w", encoding="utf-8").write(example)
            found, _, _ = run(tmp)
            return found

        clean = audit(_CLEAN)
        if clean:
            ok = False
            print("FAIL  a matching pair must audit clean, but it reported:")
            for f in clean:
                print(f"        [{f.check}] {f.message}")
        else:
            print("pass  a matching pair audits clean")

        for name, code, expect in _CASES:
            found = audit(code)
            hit = [f for f in found if f.check == expect and f.severity == "error"]
            if hit:
                print(f"pass  caught: {name}\n        → {hit[0].message}")
            else:
                ok = False
                print(f"FAIL  missed: {name} (expected a `{expect}` error, got {[f.check for f in found]})")

        for name, ref, ex in (
            ("an example rendering a different shader than the renderer does", _PARITY_REFERENCE, _PARITY_EXAMPLE),
            ("an example drawing the same vertex entry as a different primitive", _TOPOLOGY_REFERENCE, _TOPOLOGY_EXAMPLE),
        ):
            found = audit(ref, ex)
            hit = [f for f in found if f.check == "parity" and f.severity == "error"]
            if hit:
                print(f"pass  caught: {name}\n        → {hit[0].message}")
            else:
                ok = False
                print(f"FAIL  missed: {name} (got {[(f.check, f.message) for f in found]})")

    ok = _self_test_coverage() and ok

    print(
        "\nself-test: "
        + (
            "every historic drift is caught, the audit's own reach is checked, and a clean pair stays clean."
            if ok
            else "FAILED"
        )
    )
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
