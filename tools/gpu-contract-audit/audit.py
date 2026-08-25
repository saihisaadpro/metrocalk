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
  bindings     every `@group`/`@binding` it reaches exists in that group's layout, with the same
               kind, a visibility covering the stage that reaches it, and a min_binding_size that
               can hold the block — the level below `groups`, where a binding added to a group the
               layout already declared used to pass unexamined
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
# `rustgpu` reads `RenderPipelineDescriptor` and nothing else, so a compute pipeline is invisible to
# every check in this file. That is survivable while there are none — and it stops being survivable
# silently. The UNREADABLE guard below only fires for a file that parses to ZERO pipelines, so a
# compute pass added to `render.rs`, which parses fourteen render pipelines, would slip past it in
# exactly the way `@binding` slipped past `groups`. Counted separately, and reported.
COMPUTE_PIPELINE_CALL = re.compile(r"\.create_compute_pipeline\s*\(")


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


# ── WGSL resource types, as the pipeline layout has to describe them ──────────────────────────────
# The two sides spell the same fact differently: WGSL says `texture_depth_2d`, Rust says
# `TextureSampleType::Depth` + `TextureViewDimension::D2`. Both are reduced to one canonical string
# so the comparison is a string equality and not a pile of special cases.
#
# What is deliberately NOT in the canonical form is as important as what is. `filterable` and
# `has_dynamic_offset` are facts only the Rust side states — WGSL's `texture_2d<f32>` says nothing
# about either, so comparing them would manufacture a disagreement out of one side's silence.
# `multisampled` is the same in principle and worse in practice: `ssao-input-bgl` sets it from the
# run-time MSAA sample count (`multisampled: samples > 1`) while `render.rs::single_sample_ssao_source`
# rewrites the shader's own binding block to match — a pair chosen at start-up, which a reader of the
# file at rest cannot decide. The audit says so (see `_binding_findings`) rather than guessing.

_TEX_DIM = {
    "1d": "D1",
    "2d": "D2",
    "2d_array": "D2Array",
    "3d": "D3",
    "cube": "Cube",
    "cube_array": "CubeArray",
}
_STORAGE_ACCESS = {"write": "writeonly", "read": "readonly", "read_write": "readwrite"}


def wgsl_binding_kind(g: wgsl.Global) -> tuple[frozenset[str], str | None]:
    """(kinds the layout may legally declare, or an empty set with the reason it is unreadable).

    A set rather than one string because WGSL's `sampler` covers both `Filtering` and
    `NonFiltering` — the distinction lives in the sampler resource, not in the shader's type — so
    demanding one of them would be the audit inventing a rule wgpu does not have.
    """
    if g.address_space == "uniform":
        return frozenset({"buffer:uniform"}), None
    if g.address_space == "storage":
        access = g.access or "read"  # WGSL's default access for `var<storage>` is `read`
        return frozenset({"buffer:storage-read" if access == "read" else "buffer:storage-rw"}), None
    ty = g.ty
    if ty == "sampler":
        return frozenset({"sampler:filtering", "sampler:nonfiltering"}), None
    if ty == "sampler_comparison":
        return frozenset({"sampler:comparison"}), None
    m = re.fullmatch(r"texture_depth_(?:(multisampled)_)?(\w+)", ty)
    if m:
        dim = _TEX_DIM.get(m.group(2))
        return (frozenset({f"texture:depth:{dim}"}), None) if dim else (frozenset(), f"unknown depth texture `{ty}`")
    m = re.fullmatch(r"texture_(?:(multisampled)_)?(1d|2d|2d_array|3d|cube|cube_array)<(\w+)>", ty)
    if m:
        scalar = {"f32": "float", "u32": "uint", "i32": "sint"}.get(m.group(3))
        dim = _TEX_DIM[m.group(2)]
        return (frozenset({f"texture:{scalar}:{dim}"}), None) if scalar else (frozenset(), f"unknown sampled type in `{ty}`")
    m = re.fullmatch(r"texture_storage_(1d|2d|2d_array|3d|cube|cube_array)<[^,]+,(\w+)>", ty)
    if m:
        access = _STORAGE_ACCESS.get(m.group(2))
        dim = _TEX_DIM[m.group(1)]
        return (
            (frozenset({f"storage-texture:{access}:{dim}"}), None)
            if access
            else (frozenset(), f"unknown storage-texture access in `{ty}`")
        )
    return frozenset(), f"resource type `{ty}` is not one this audit knows"


def _size_of(expr: str, rf: rustgpu.RustFile) -> int | None:
    """`std::mem::size_of::<Camera>() as u64` → the `#[repr(C)]` size this audit already computes."""
    m = re.search(r"size_of\s*::\s*<\s*([A-Za-z_]\w*)\s*>", expr)
    if not m or m.group(1) not in rf.structs:
        return None
    got = rust_struct_size(rf.structs[m.group(1)].fields, rf.structs)
    return None if got is None else got[1]


def _resolve_bgl(
    name: str, rf: rustgpu.RustFile, index: dict[str, tuple[str, rustgpu.BindGroupLayout, rustgpu.RustFile]]
) -> tuple[str, rustgpu.BindGroupLayout, rustgpu.RustFile] | None:
    """A bind-group-layout identifier → the file and declaration it came from.

    Local first, then the alias (`let ibl_bgl = crate::ibl::bind_group_layout(&device);`), then the
    cross-file index by bare name. Group 3 of the mesh pipeline is built in another file entirely,
    and a resolver that stopped at file scope would report "not declared here" about a layout that
    is perfectly well declared — a false finding, which erodes a gate as surely as a missed one.
    """
    if name in rf.bind_group_layouts:
        return rf.path, rf.bind_group_layouts[name], rf
    alias = rf.bgl_aliases.get(name)
    if alias and alias in index:
        return index[alias]
    if name in index:
        return index[name]
    return None


def _binding_findings(
    where: str,
    p: rustgpu.Pipeline,
    rf: rustgpu.RustFile,
    mod: wgsl.Module,
    reached: dict[str, tuple[wgsl.Global, frozenset[str]]],
    index: dict[str, tuple[str, rustgpu.BindGroupLayout, rustgpu.RustFile]],
) -> tuple[list[Finding], int]:
    """Every `@group(G) @binding(B)` the shader reaches, against the layout entry that answers it.

    The `groups` check is `@group`-level — it asks whether the pipeline layout declares enough bind
    groups. A binding added inside a group the layout already declares is below its resolution, and
    that is not a hypothetical: `@group(3) @binding(6)` (the SH irradiance uniform, ADR-042 follow-on)
    was added, verified by hand, and passed unexamined. wgpu rejects a drifted binding — at pipeline
    creation, on a GPU host, in a build that got that far.
    """
    out: list[Finding] = []
    if p.layout is None or p.layout not in rf.pipeline_layouts:
        return out, 0  # already reported by `groups`; one defect, one finding
    groups = rf.pipeline_layouts[p.layout].groups
    compared = 0
    seen_layouts: set[str] = set()
    for name, (g, stages) in sorted(reached.items(), key=lambda kv: (kv[1][0].group, kv[1][0].binding)):
        if g.group >= len(groups):
            continue  # `groups` says this louder, and with the right repair
        found = _resolve_bgl(groups[g.group], rf, index)
        if found is None:
            out.append(
                Finding(
                    "unresolved",
                    "bindings",
                    where,
                    f"binds `{groups[g.group]}` at @group({g.group}), and no bind group layout by that "
                    f"name was parsed in this file or any other — the bindings inside it are UNCHECKED, "
                    f"not clean. Teach rustgpu.py the shape it is built with",
                )
            )
            continue
        owner_rel, bgl, owner_rf = found
        site = f"{owner_rel}:{bgl.line} [{bgl.label or bgl.name}]"
        if bgl.unresolved and site not in seen_layouts:
            seen_layouts.add(site)
            for msg in bgl.unresolved:
                out.append(Finding("unresolved", "bindings", site, msg))
        entry = next((e for e in bgl.entries if e.binding == g.binding), None)
        if entry is None:
            unread = [e for e in bgl.entries if e.binding is None]
            out.append(
                Finding(
                    "unresolved" if unread else "error",
                    "bindings",
                    site,
                    f"`{p.label}` reaches @group({g.group}) @binding({g.binding}) `{name}` "
                    f"({g.address_space} {g.ty}), and this layout declares "
                    + (
                        f"{len(unread)} entry/entries whose binding index this audit could not read"
                        if unread
                        else f"bindings {sorted(e.binding for e in bgl.entries) or '(none)'}"
                    ),
                )
            )
            continue
        if entry.unresolved:
            for msg in entry.unresolved:
                out.append(Finding("unresolved", "bindings", f"{owner_rel}:{entry.line}", msg))
            continue
        compared += 1
        want, why = wgsl_binding_kind(g)
        if not want:
            out.append(Finding("unresolved", "bindings", site, f"@binding({g.binding}) `{name}`: {why}"))
            continue
        if entry.kind not in want:
            out.append(
                Finding(
                    "error",
                    "bindings",
                    site,
                    f"@group({g.group}) @binding({g.binding}) is `{entry.kind}` in the layout but "
                    f"`{name}: {g.ty}` in the shader, which needs "
                    f"{' or '.join(sorted('`%s`' % k for k in want))}",
                )
            )
        missing = stages - entry.visibility
        if missing:
            out.append(
                Finding(
                    "error",
                    "bindings",
                    site,
                    f"@group({g.group}) @binding({g.binding}) `{name}` is visible to "
                    f"{'/'.join(sorted(entry.visibility)) or '(no stage)'}, but `{p.label}` reaches it from "
                    f"{'/'.join(sorted(missing))} — wgpu: \"Visibility flags don't include the shader stage\"",
                )
            )
        mbs = entry.min_binding_size
        if isinstance(mbs, str):
            # Declared, but not a literal. `size_of::<T>()` over a `#[repr(C)]` struct this audit
            # already sizes is resolved; anything else is reported rather than skipped, because a
            # size the gate could not read must not print the same nothing as a size that agreed.
            resolved = _size_of(mbs, owner_rf) or _size_of(mbs, rf)
            if resolved is None:
                out.append(
                    Finding(
                        "unresolved",
                        "bindings",
                        site,
                        f"@group({g.group}) @binding({g.binding}) declares min_binding_size `{mbs[:60]}`, "
                        f"an expression this audit cannot evaluate — the size half of this binding is "
                        f"UNCHECKED, not agreed. Teach rustgpu.py the shape it is written in",
                    )
                )
            mbs = resolved
        if mbs is not None and g.address_space in ("uniform", "storage"):
            try:
                size = wgsl.layout_of(g.ty, mod.structs).size
            except wgsl.WgslError:
                size = 0
            if size and mbs < size:
                out.append(
                    Finding(
                        "error",
                        "bindings",
                        site,
                        f"@group({g.group}) @binding({g.binding}) declares min_binding_size "
                        f"{mbs}, but the shader's `{g.ty}` block is {size} bytes — "
                        f"a binding smaller than the block the shader indexes",
                    )
                )
    return out, compared


_LAYOUT_CLASS = {"buffer": "buffer", "sampler": "sampler", "texture": "texture", "storage-texture": "texture"}


def _resource_findings(
    rel: str,
    rf: rustgpu.RustFile,
    resolver: rustgpu.LayoutResolver,
    index: dict[str, tuple[str, rustgpu.BindGroupLayout, rustgpu.RustFile]],
) -> tuple[list[Finding], int]:
    """Every `wgpu::BindGroupEntry` against the layout entry that has to accept it.

    `bindings` compares the SHADER with the LAYOUT. This compares the LAYOUT with the RESOURCES —
    the third statement of the same contract, checked by wgpu at `create_bind_group` rather than at
    `create_render_pipeline`, which is a different moment and a different failure. A `TextureView`
    bound where the layout declares a `Buffer` is invisible to every check above it: proved, not
    assumed, by six such drifts on a copy of this crate that the audit passed
    (`progress/gpu-bind-group-resources/02-before-mutation-matrix.txt`).

    Granularity, stated so it cannot be mistaken for more: RESOURCE CLASS and BINDING INDEX SET. A
    `BindingResource` says "a buffer" and not which usage flags that buffer carries, so a UNIFORM
    buffer bound to a STORAGE layout entry is still below this check — wgpu resolves that against
    `create_buffer`, a third file neither of these two holds. (`<test_and_ci_discipline>` 6b: write
    down the resolution a check works at, or the next fact added below it passes unexamined.)
    """
    out: list[Finding] = []
    compared = 0
    for bg in rf.bind_groups:
        where = f"{rel}:{bg.line} [{bg.label or '<unlabelled>'}]"
        for msg in bg.unresolved:
            out.append(Finding("unresolved", "resources", where, msg))
        for e in bg.entries:
            for msg in e.unresolved:
                out.append(Finding("unresolved", "resources", f"{rel}:{e.line}", msg))

        seen: dict[int, int] = {}
        for e in bg.entries:
            if e.binding is None:
                continue
            if e.binding in seen:
                out.append(
                    Finding(
                        "error",
                        "resources",
                        where,
                        f"declares @binding({e.binding}) twice (lines {seen[e.binding]} and {e.line}) — "
                        'wgpu: "Duplicate binding index"',
                    )
                )
            seen[e.binding] = e.line

        names, why = resolver.resolve(rel, bg.at, bg.layout_expr)
        for msg in why:
            out.append(
                Finding(
                    "unresolved",
                    "resources",
                    where,
                    f"its `layout: {bg.layout_expr[:40]}` could not be resolved to a bind group layout — "
                    f"{msg}. The resources in this group are UNCHECKED, not clean",
                )
            )
        if why or not names:
            if not names and not why:
                out.append(
                    Finding(
                        "unresolved",
                        "resources",
                        where,
                        f"its `layout: {bg.layout_expr[:40]}` resolved to no bind group layout at all",
                    )
                )
            continue

        for name in sorted(names):
            found = _resolve_bgl(name, rf, index)
            if found is None:
                out.append(
                    Finding(
                        "unresolved",
                        "resources",
                        where,
                        f"binds layout `{name}`, which no file declares in a shape this audit can read",
                    )
                )
                continue
            owner_rel, bgl, _ = found
            site = f"{name} ({owner_rel}:{bgl.line})"
            if bgl.unresolved or any(e.binding is None for e in bgl.entries):
                continue  # `bindings` already reports this layout; one defect, one finding
            declared = {e.binding: e for e in bgl.entries}
            for e in bg.entries:
                if e.binding is None or not e.kind:
                    continue
                le = declared.get(e.binding)
                if le is None:
                    out.append(
                        Finding(
                            "error",
                            "resources",
                            where,
                            f"binds @binding({e.binding}) (a {e.kind}), and {site} declares bindings "
                            f"{sorted(declared) or '(none)'} — wgpu rejects a bind group entry the "
                            "layout has no slot for",
                        )
                    )
                    continue
                compared += 1
                want = _LAYOUT_CLASS.get(le.kind.split(":")[0])
                if want is None:
                    continue  # an unreadable layout kind; `bindings` says it
                if want != e.kind:
                    out.append(
                        Finding(
                            "error",
                            "resources",
                            where,
                            f"binds a {e.kind} at @binding({e.binding}), but {site} declares "
                            f"`{le.kind}` there — wgpu: \"Binding {e.binding} entry type mismatch\"",
                        )
                    )
            missing = sorted(set(declared) - {e.binding for e in bg.entries})
            if missing:
                out.append(
                    Finding(
                        "error",
                        "resources",
                        where,
                        f"supplies {len(bg.entries)} entr(ies) and {site} declares "
                        f"{len(declared)}: nothing is bound at @binding "
                        f"{', @binding '.join(str(b) for b in missing)} — wgpu requires the bind "
                        "group to answer every entry in its layout",
                    )
                )
    return out, compared


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
    bgl_index: dict[str, tuple[str, rustgpu.BindGroupLayout, rustgpu.RustFile]] | None = None,
    stats: dict[str, int] | None = None,
) -> list[Finding]:
    out: list[Finding] = []
    index = bgl_index if bgl_index is not None else {}
    counts = stats if stats is not None else {}

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
        reached: dict[str, tuple[wgsl.Global, frozenset[str]]] = {
            n: (g, frozenset({"VERTEX"})) for n, g in used.items()
        }
        if fs is not None:
            for n, g in mod.resources_used(fs[1].name).items():
                used[n] = g
                prior = reached[n][1] if n in reached else frozenset()
                reached[n] = (g, prior | {"FRAGMENT"})
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

        # ── bindings ───────────────────────────────────────────────────────────────────────────
        found, compared = _binding_findings(where, p, rf, mod, reached, index)
        out.extend(found)
        counts["bindings"] = counts.get("bindings", 0) + compared

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


_MARKER = re.compile(r"^\s*//\s*>>>\s*(.+?)\s*<<<\s*$")


def shader_variants(
    rf: rustgpu.RustFile, sources: dict[str, str]
) -> tuple[dict[str, tuple[str, str]], list[Finding]]:
    """The shader modules this file BUILDS AT RUN TIME by marker substitution.

    `{variant key: (base shader name, variant source)}`, discovered from the source rather than
    configured: a `const` whose value is a `// >>> X <<<` marker, its `// >>> END X <<<` partner, and
    a `const` whose value is WGSL. Any shader holding both markers gets the substitution applied
    exactly as `single_sample_ssao_source` applies it — start marker inclusive through end marker
    inclusive.

    A marker pair the audit cannot complete is a FINDING, not a skip: a shader that reaches the GPU
    in a shape no file states is the whole of what this function exists to stop.
    """
    consts = rustgpu.string_consts(rf.src)
    markers = {n: _MARKER.match(v).group(1) for n, v in consts.items() if _MARKER.match(v)}
    replacements = {n: v for n, v in consts.items() if n not in markers and "@group(" in v}
    pairs = [
        (consts[s], consts[e])
        for s, ns in markers.items()
        for e, ne in markers.items()
        if ne == f"END {ns}"
    ]

    out: dict[str, tuple[str, str]] = {}
    findings: list[Finding] = []
    for name, text in sources.items():
        opens = [ln for ln in text.splitlines() if _MARKER.match(ln)]
        if not opens:
            continue
        applicable = [(s, e) for s, e in pairs if s in text and e in text]
        if not applicable or not replacements:
            findings.append(
                Finding(
                    "unresolved",
                    "entries",
                    f"{rf.path} [{name}]",
                    f"{name} carries {len(opens)} `// >>> … <<<` substitution marker(s), and this audit "
                    f"could not build the substituted variant ({len(pairs)} marker pair(s), "
                    f"{len(replacements)} WGSL const(s) found in {rf.path}) — the shader the host "
                    "actually compiles is UNAUDITED. It is the file at rest that is being checked, "
                    "not the module that reaches the GPU",
                )
            )
            continue
        for start, end in applicable:
            a, b = text.find(start), text.find(end)
            if a < 0 or b < a:
                continue
            for cname, body in replacements.items():
                out[f"{name} [{cname}]"] = (name, text[:a] + body + text[b + len(end) :])
    return out, findings


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
    stats: dict[str, int] | None = None,
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

    # 1b. A compute pipeline, in a file that also builds render pipelines. `rustgpu` reads only
    #     `RenderPipelineDescriptor`, so the compute pass's entry point, bind groups and bindings are
    #     unchecked — and because the file parses plenty of *render* pipelines, guard 1 above has
    #     nothing to complain about. Say it here instead of discovering it the way `@binding` was.
    for rel, raw in scanned:
        n = len(COMPUTE_PIPELINE_CALL.findall(re.sub(r"//[^\n]*", "", raw)))
        if n:
            out.append(
                Finding(
                    "unresolved",
                    "coverage",
                    rel,
                    f"builds {n} compute pipeline(s), and this audit reads `RenderPipelineDescriptor` "
                    "only — their entry points, bind groups and bindings are UNCHECKED. Teach "
                    "rustgpu.py `ComputePipelineDescriptor`, or the first compute pass in this "
                    "renderer arrives with no contract check at all",
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

    # 3. The `bindings` check, checked. A tree with pipelines and shaders that reach resources, and
    #    zero bindings compared, means the layouts were not read — the same "0 findings" as agreement.
    #    This is the guard the `groups` check never had, and the reason it took a hand-check to notice
    #    that a whole binding could be added below its resolution.
    if stats is not None and stats.get("bindings", 0) == 0:
        reaches = any(
            m.resources_used(e) for _, _, shaders in parsed for m in shaders.values() for e in m.entries
        )
        if reaches and any(rf.pipelines for _, rf, _ in parsed):
            out.append(
                Finding(
                    "error",
                    "coverage",
                    root.replace(os.sep, "/"),
                    f"the shaders reach `@group`/`@binding` resources and the audit compared 0 of them "
                    f"against a layout entry ({stats.get('layouts', 0)} bind group layout(s) parsed) — the "
                    "`bindings` check is switched off, not passing",
                )
            )

    # 4. The same guard for `resources`, and a stricter one beside it. `bindings` can be silenced by
    #    reading no layouts; `resources` can be silenced by reading no bind GROUPS — and a
    #    `create_bind_group` call that parses to nothing is the `mesh_frame_bench` shape exactly: a
    #    file that compiles, an audit that says nothing, and a contract with no reader.
    for rel, raw in scanned:
        stripped = re.sub(r"//[^\n]*", "", raw)
        calls = len(re.findall(r"\.create_bind_group\s*\(", stripped))
        got = next((len(rf.bind_groups) for r, rf, _ in parsed if r == rel), 0)
        if calls > got:
            out.append(
                Finding(
                    "unresolved",
                    "coverage",
                    rel,
                    f"calls create_bind_group {calls} time(s) and the audit parsed {got} bind group(s) "
                    "from it — the resources in the difference are UNREADABLE to the gate, not clean. "
                    "Teach rustgpu.py the descriptor shape it uses",
                )
            )
    if stats is not None and stats.get("resources", 0) == 0 and stats.get("bind_groups", 0):
        out.append(
            Finding(
                "error",
                "coverage",
                root.replace(os.sep, "/"),
                f"{stats['bind_groups']} bind group(s) were parsed and 0 of their entries were compared "
                "against a layout entry — the `resources` check is switched off, not passing",
            )
        )
    return out


def run(root: str) -> tuple[list[Finding], list[str], list[str], dict[str, int]]:
    findings: list[Finding] = []
    audited: list[str] = []
    cache: dict[str, wgsl.Module] = {}
    text_cache: dict[str, str] = {}
    parsed: list[tuple[str, rustgpu.RustFile, dict[str, wgsl.Module]]] = []
    variants: dict[str, dict[str, tuple[str, wgsl.Module]]] = {}
    scanned: list[tuple[str, str]] = []

    for path in _rust_files(root):
        rel = os.path.relpath(path, root).replace(os.sep, "/")
        raw = open(path, encoding="utf-8").read()
        scanned.append((rel, raw))
        rf = rustgpu.parse_rust(rel, raw)
        # A file with no pipeline and no `#[repr(C)]` struct used to be dropped here — which silently
        # dropped `ibl.rs`, whose only contribution is the group-3 bind group layout the mesh and sky
        # pipelines both bind. The layout has to be read from the file that declares it, not the file
        # that uses it.
        if not rf.pipelines and not rf.structs and not rf.bind_group_layouts:
            continue
        shaders: dict[str, wgsl.Module] = {}
        texts: dict[str, str] = {}
        for inc in rf.shader_sources:
            spath = os.path.normpath(os.path.join(os.path.dirname(path), inc))
            key = os.path.basename(spath)
            if spath not in cache:
                try:
                    text_cache[spath] = open(spath, encoding="utf-8").read()
                    cache[spath] = wgsl.parse(text_cache[spath])
                except (OSError, wgsl.WgslError) as e:
                    findings.append(Finding("unresolved", "entries", rel, f"cannot read {inc}: {e}"))
                    continue
            shaders[key] = cache[spath]
            texts[key] = text_cache[spath]
        if not rf.pipelines and not shaders and not rf.bind_group_layouts:
            continue

        # The shaders this file BUILDS rather than includes. `ssao.wgsl` on disk is the MSAA-on
        # module; the MSAA-off one is assembled at start-up and has never been read by anything.
        built, marker_findings = shader_variants(rf, texts)
        findings.extend(marker_findings)
        for vkey, (base, vsrc) in built.items():
            try:
                variants.setdefault(rel, {})[vkey] = (base, wgsl.parse(vsrc))
            except wgsl.WgslError as e:
                findings.append(
                    Finding(
                        "error",
                        "entries",
                        f"{rel} [{vkey}]",
                        f"the substituted variant of {base} does not parse as WGSL: {e}. The host "
                        "compiles this module at start-up",
                    )
                )
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

    # Bind group layouts are indexed across the whole tree, not per file: `mesh_layout`'s group 3 is
    # `crate::ibl::bind_group_layout(&device)`, declared in another module. A per-file resolver would
    # have to call that "not declared" — a false finding about a layout that is perfectly well
    # declared, which costs a gate its credibility as fast as a missed one does.
    bgl_index: dict[str, tuple[str, rustgpu.BindGroupLayout, rustgpu.RustFile]] = {}
    for rel, rf, _ in parsed:
        for name, bgl in rf.bind_group_layouts.items():
            bgl_index.setdefault(name, (rel, bgl, rf))

    # The `layout:` of a bind group is a parameter at ten of this renderer's fifteen construction
    # sites, and one of them is two calls and one module away from the layout it gets. The resolver
    # substitutes across call sites, so it needs every file at once — not one at a time.
    resolver = rustgpu.LayoutResolver({rel: rf.src for rel, rf, _ in parsed}, set(bgl_index))

    stats: dict[str, int] = {
        "bindings": 0,
        "layouts": len(bgl_index),
        "resources": 0,
        "bind_groups": sum(len(rf.bind_groups) for _, rf, _ in parsed),
        "variants": sum(len(v) for v in variants.values()),
    }
    for rel, rf, shaders in parsed:
        audited.append(rel)
        base = check_file(
            rf,
            shaders,
            rel,
            reference if rel.startswith("examples/") else None,
            bgl_index=bgl_index,
            stats=stats,
        )
        findings.extend(base)

        # Each run-time variant is audited as the module it replaces, so every check above it —
        # entries, attributes, stages, groups, bindings — runs against the source the host actually
        # compiles. Findings identical to the base pass are the base's; only what the SUBSTITUTION
        # introduced is new, and it is labelled with the const that introduced it.
        seen = {(f.severity, f.check, f.where, f.message) for f in base}
        for vkey, (bname, vmod) in variants.get(rel, {}).items():
            for f in check_file(
                rf,
                {**shaders, bname: vmod},
                rel,
                reference if rel.startswith("examples/") else None,
                bgl_index=bgl_index,
                stats=None,  # the base pass owns the counts; a variant must not inflate them
            ):
                if (f.severity, f.check, f.where, f.message) in seen:
                    continue
                findings.append(
                    Finding(f.severity, f.check, f.where, f"in the run-time variant `{vkey}`: {f.message}")
                )

        res_findings, n = _resource_findings(rel, rf, resolver, bgl_index)
        findings.extend(res_findings)
        stats["resources"] += n

    findings.extend(_coverage_findings(root, scanned, parsed, reference, stats))

    seen: set[tuple[str, str, str, str]] = set()
    unique: list[Finding] = []
    for f in findings:
        key = (f.severity, f.check, f.where, f.message)
        if key not in seen:
            seen.add(key)
            unique.append(f)
    return unique, audited, [rel for rel, _ in scanned], stats


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

    findings, audited, scanned, stats = run(args.root)
    waivers = {} if args.no_waivers else read_waivers(args.root)

    if args.json:
        print(
            json.dumps(
                {
                    "audited": audited,
                    "scanned": scanned,
                    "stats": stats,
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
    # The same argument as "N of M scanned", one level down: a `bindings` count of 0 and a `bindings`
    # count of 47 both print no findings, and only one of them is a check that ran.
    print(
        f"                    {stats.get('bindings', 0)} shader binding(s) compared against "
        f"{stats.get('layouts', 0)} bind group layout(s)"
    )
    # Resource CLASS and binding INDEX, which is all a `BindingResource` states — see
    # `_resource_findings`. Saying the granularity next to the count is the point: the previous
    # version of this gate printed a number that looked like whole coverage of a group it only
    # reached the surface of.
    print(
        f"                    {stats.get('resources', 0)} bind group entr(ies) compared against their "
        f"layout, by resource class, across {stats.get('bind_groups', 0)} bind group(s)"
    )
    print(
        f"                    {stats.get('variants', 0)} run-time shader variant(s) rebuilt from their "
        f"substitution markers and audited"
    )
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

# The bind group layouts `_SHADER`'s three groups need. Every fixture below carries them, because
# `bindings` reads the layout ENTRIES and a pipeline layout naming three identifiers that are
# declared nowhere is — correctly — an UNREAD, not a clean pair. Writing the fixtures without these
# was the first thing the new check found, in its own test data.
_BGLS = """
fn layouts(device: &D) {
    let a = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("cam"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    });
    let b = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("inst"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    });
    let c = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("lights"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    });
}
"""

_CASES: list[tuple[str, str, str]] = [
    (
        "a vertex attribute the shader grew and the host never supplied",
        _BGLS + """
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
        _BGLS + """
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
        _BGLS + """
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
        _BGLS + """
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

_CLEAN = _BGLS + """
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
_PARITY_REFERENCE = _BGLS + """
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

_PARITY_EXAMPLE = _BGLS + """
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
_TOPOLOGY_REFERENCE = _BGLS + """
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

_TOPOLOGY_EXAMPLE = _BGLS + """
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
            found, _, _, _ = run(root)
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

    # 7. A compute pipeline beside the render pipelines. The file parses fine and audits clean for
    #    everything this tool reads — which is nothing about the compute pass. The renderer has no
    #    compute pipeline today; this case is what stops the first one from arriving unchecked.
    def _compute_beside_render(tmp: str) -> str:
        src = os.path.join(tmp, "src")
        os.makedirs(src)
        open(os.path.join(src, "s.wgsl"), "w", encoding="utf-8").write(_SHADER)
        open(os.path.join(src, "case.rs"), "w", encoding="utf-8").write(
            _PARITY_REFERENCE
            + "\nfn cull(device: &D) { let p = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor "
            '{ label: Some("cull"), layout: Some(&lay), module: &s, entry_point: Some("cs_cull") }); }\n'
        )
        return tmp

    case(
        "a compute pipeline in a file whose render pipelines all parse",
        _compute_beside_render,
        "coverage",
        "reads `RenderPipelineDescriptor` only",
        "unresolved",
    )
    return ok


# ── the `bindings` family ─────────────────────────────────────────────────────────────────────────
# One binding, drifted in every direction it can drift, plus the shapes the layout is built with.
# The subject is the real one: `@group(3) @binding(6) var<uniform> sh: Irradiance` — 144 bytes of
# cosine-convolved SH irradiance (Ramamoorthi & Hanrahan, SIGGRAPH 2001), added to a group the
# pipeline layout already declared, verified by a human reading two files side by side, and passed
# by a gate whose `groups` check could not see below `@group`. Every case here reported `0 blocking`
# before this check existed: `progress/gpu-bindings-check/02-before-mutation-matrix.txt`.

_BIND_SHADER = """
struct Camera { view_proj: mat4x4<f32> };
@group(0) @binding(0) var<uniform> cam: Camera;
struct Irradiance { coeff: array<vec4<f32>, 9> };
@group(1) @binding(0) var env: texture_2d<f32>;
@group(1) @binding(1) var env_samp: sampler;
@group(1) @binding(2) var shadow_map: texture_depth_2d;
@group(1) @binding(3) var shadow_samp: sampler_comparison;
@group(1) @binding(6) var<uniform> sh: Irradiance;
struct IblOut { @builtin(position) pos: vec4<f32>, @location(0) n: vec3<f32> };
@vertex
fn vs_ibl(@location(0) p: vec3<f32>) -> IblOut {
    var o: IblOut; o.pos = cam.view_proj * vec4<f32>(p, 1.0); o.n = p; return o;
}
@fragment
fn fs_ibl(in: IblOut) -> @location(0) vec4<f32> {
    let e = textureSample(env, env_samp, in.n.xy);
    let s = textureSampleCompare(shadow_map, shadow_samp, in.n.xy, 0.5);
    return e * s * sh.coeff[0];
}
"""

# The renderer: group 0 built here through a parameterised helper (`bgl_entry`), group 1 built in
# another module entirely — both shapes the real `render.rs` uses, and both invisible to a resolver
# that reads one file at a time or takes `-> wgpu::BindGroupLayoutEntry` for a struct literal.
_BIND_RENDERER = """
const S: &str = include_str!("s.wgsl");
fn build(device: &D) {
    let cam_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("cam-bgl"),
        entries: &[bgl_entry(0, wgpu::ShaderStages::VERTEX_FRAGMENT, wgpu::BufferBindingType::Uniform)],
    });
    let ibl_bgl = crate::ibl::bind_group_layout(&device);
    let vbl = wgpu::VertexBufferLayout { array_stride: 12, step_mode: m, attributes: &[
        wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 0, shader_location: 0 },
    ]};
    let lay = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None, bind_group_layouts: &[Some(&cam_bgl), Some(&ibl_bgl)], immediate_size: 0 });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor { label: Some("ibl"),
        layout: Some(&lay),
        vertex: wgpu::VertexState { module: &s, entry_point: Some("vs_ibl"),
            buffers: std::slice::from_ref(&vbl), compilation_options: d },
        fragment: Some(wgpu::FragmentState { module: &s, entry_point: Some("fs_ibl"),
            targets: &[], compilation_options: d }), });
}
fn bgl_entry(
    binding: u32,
    vis: wgpu::ShaderStages,
    ty: wgpu::BufferBindingType,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: vis,
        ty: wgpu::BindingType::Buffer { ty, has_dynamic_offset: false, min_binding_size: None },
        count: None,
    }
}
"""

_BIND_IBL = """
pub fn bind_group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    let tex = |binding| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    };
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("ibl-bgl"),
        entries: &[
            tex(0),
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 3,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 6,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
        ],
    })
}
"""

_SH_ENTRY = """            wgpu::BindGroupLayoutEntry {
                binding: 6,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
"""


def _self_test_bindings() -> bool:
    import tempfile

    ok = True

    def run_case(
        name: str,
        renderer: str = _BIND_RENDERER,
        ibl: str = _BIND_IBL,
        shader: str = _BIND_SHADER,
        want: tuple[str, str, str, str | None] | None = None,
    ) -> None:
        """`want` = (check, severity, message substring, `where` prefix or None). None = must be clean."""
        nonlocal ok
        with tempfile.TemporaryDirectory() as tmp:
            src = os.path.join(tmp, "src")
            os.makedirs(src)
            open(os.path.join(src, "s.wgsl"), "w", encoding="utf-8").write(shader)
            open(os.path.join(src, "case.rs"), "w", encoding="utf-8").write(renderer)
            open(os.path.join(src, "ibl.rs"), "w", encoding="utf-8").write(ibl)
            found, _, _, stats = run(tmp)
            if want is None:
                if found or stats["bindings"] == 0:
                    ok = False
                    print(
                        f"FAIL  {name}: expected a clean pair with bindings compared, got "
                        f"{stats['bindings']} compared and {[(f.check, f.message[:70]) for f in found]}"
                    )
                else:
                    print(f"pass  {name} ({stats['bindings']} binding(s) compared, 0 findings)")
                return
            check, severity, text, where_prefix = want
            hit = [
                f
                for f in found
                if f.check == check
                and f.severity == severity
                and text in f.message
                and (where_prefix is None or f.where.startswith(where_prefix))
            ]
            if hit:
                print(f"pass  caught: {name}\n        → {hit[0].where}: {hit[0].message}")
            else:
                ok = False
                print(
                    f"FAIL  missed: {name} (expected a `{check}`/{severity} at {where_prefix} saying "
                    f"{text!r}, got {[(f.check, f.severity, f.where, f.message[:70]) for f in found]})"
                )

    # 0. The reference pair. Both construction shapes resolved, 6 bindings compared, nothing to say.
    #    `stats["bindings"] > 0` is part of the assertion: a clean run and a run that compared
    #    nothing print the same 0 findings, which is the failure this whole check was written about.
    run_case("a matching layout audits clean, and is actually compared")

    ibl_at = "src/ibl.rs"

    # 1. The binding the shader reaches, gone from the layout. wgpu at pipeline creation:
    #    "Shader global ... is not available in the layout".
    run_case(
        "a binding the shader reaches that the layout does not declare",
        ibl=_BIND_IBL.replace(_SH_ENTRY, ""),
        want=("bindings", "error", "this layout declares bindings [0, 1, 2, 3]", ibl_at),
    )

    # 2. The same binding, right index, wrong kind.
    run_case(
        "a uniform block the layout declares as a texture",
        ibl=_BIND_IBL.replace(
            """                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },""",
            """                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },""",
        ),
        want=("bindings", "error", "is `texture:float:D2` in the layout", ibl_at),
    )

    # 3. Right index, right kind, wrong stage. wgpu: "Visibility flags don't include the shader
    #    stage" — the drift `mesh_frame_bench`'s own camera layout carried for four generations.
    run_case(
        "a binding the fragment stage reaches and the layout makes vertex-only",
        ibl=_BIND_IBL.replace(_SH_ENTRY, _SH_ENTRY.replace("ShaderStages::FRAGMENT", "ShaderStages::VERTEX")),
        want=("bindings", "error", "reaches it from FRAGMENT", ibl_at),
    )

    # 4. The drift from the other side: the shader moves and the layout stays.
    run_case(
        "the shader moving a binding index the layout still answers at the old one",
        shader=_BIND_SHADER.replace("@binding(6) var<uniform> sh", "@binding(7) var<uniform> sh"),
        want=("bindings", "error", "@group(1) @binding(7)", ibl_at),
    )

    # 5. The size half: a min_binding_size that cannot hold the block the shader indexes.
    run_case(
        "a min_binding_size smaller than the shader's uniform block",
        ibl=_BIND_IBL.replace(
            "min_binding_size: None,\n                },\n                count: None,\n            },\n        ],",
            "min_binding_size: std::num::NonZeroU64::new(128),\n                },\n                count: None,\n            },\n        ],",
        ),
        want=("bindings", "error", "min_binding_size 128, but the shader's `Irradiance` block is 144", ibl_at),
    )

    # 6. A depth texture the layout describes as a float one — legal-looking, and a pipeline that
    #    cannot exist. Pinned separately from case 2 because it goes through the texture branch,
    #    where the sample type and the view dimension are two independent chances to be wrong.
    run_case(
        "a depth texture the layout declares as a sampled float texture",
        ibl=_BIND_IBL.replace("wgpu::TextureSampleType::Depth", "wgpu::TextureSampleType::Float { filterable: true }"),
        want=("bindings", "error", "`shadow_map: texture_depth_2d`", ibl_at),
    )

    # 7. A comparison sampler declared as a filtering one.
    run_case(
        "a comparison sampler the layout declares as filtering",
        ibl=_BIND_IBL.replace("wgpu::SamplerBindingType::Comparison", "wgpu::SamplerBindingType::Filtering"),
        want=("bindings", "error", "needs `sampler:comparison`", ibl_at),
    )

    # 8. The helper-built entry, drifted. Group 0 comes from `bgl_entry(0, VIS, TY)`, a parameterised
    #    builder — and `fn bgl_entry(..) -> wgpu::BindGroupLayoutEntry {` matches the same text a
    #    struct literal does, so a reader that does not tell a return type from a value parses this
    #    entry as three unresolved fields. This case is pinned to the VISIBILITY message rather than
    #    to any finding, because the unresolved-fields failure is loud too and would otherwise let
    #    the case pass while the mechanism it exists for was broken.
    run_case(
        "a parameter-built entry whose visibility the vertex stage needs",
        renderer=_BIND_RENDERER.replace("wgpu::ShaderStages::VERTEX_FRAGMENT", "wgpu::ShaderStages::FRAGMENT"),
        want=("bindings", "error", "reaches it from VERTEX", "src/case.rs"),
    )

    # 9. The layout built in a shape the parser cannot read. Not "clean" — UNREAD, and loudly, or a
    #    fifth construction shape would silently switch this check off for every binding inside it.
    run_case(
        "a layout built in a shape the parser cannot read",
        renderer=_BIND_RENDERER.replace(
            "let ibl_bgl = crate::ibl::bind_group_layout(&device);", "let ibl_bgl = layouts_from!(device, 3);"
        ),
        ibl="pub fn unrelated() -> u32 { 3 }\n",
        want=("bindings", "unresolved", "the bindings inside it are UNCHECKED", "src/case.rs"),
    )

    # 10. `ssao-input-bgl`'s real shape: a depth entry whose `multisampled` follows the run-time MSAA
    #     sample count. Two things are pinned here, and the second is why this case exists at all.
    #     One: `multisampled` is NOT compared — the shader's type does not state it, and the host
    #     picks the pair at start-up (`single_sample_ssao_source` rewrites the shader's own binding
    #     block when MSAA is off), so a reader of the files at rest cannot decide it. The rest of the
    #     entry is still compared, and the assertion says so by requiring a non-zero count.
    #     Two: `samples > 1` contains a `>` that closes nothing. `_top_level_spans` used to count it
    #     as a bracket, which drove the depth negative and truncated the whole `entries:` array
    #     mid-expression — the layout parsed to ZERO entries, and every binding in it would then be
    #     reported missing. That regression is invisible to every other case here, because no other
    #     fixture contains a comparison operator.
    run_case(
        "a layout entry whose multisampled flag is a run-time comparison",
        ibl=_BIND_IBL.replace(
            """                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,""",
            """                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: samples > 1,""",
        ),
    )

    # 11. And the check on the check: shaders that reach bindings, and not one of them compared.
    run_case(
        "shaders that reach bindings with nothing compared against a layout",
        renderer=_BIND_RENDERER.replace(
            "bind_group_layouts: &[Some(&cam_bgl), Some(&ibl_bgl)]", "bind_group_layouts: &[Some(&x), Some(&y)]"
        ),
        ibl="pub fn unrelated() -> u32 { 3 }\n",
        want=("coverage", "error", "the `bindings` check is switched off, not passing", None),
    )
    return ok


# ── the `resources` family ────────────────────────────────────────────────────────────────────────
# The same contract a third time: the resources actually handed to `create_bind_group`. wgpu compares
# them against the layout at a different moment from the pipeline check, so a class swap here is
# invisible to `bindings` no matter how deep that check goes. Six such drifts were applied to a copy
# of the real crate before this family existed and the audit passed all six
# (`progress/gpu-bind-group-resources/02-before-mutation-matrix.txt`).
#
# The fixture is deliberately the awkward shape, not the easy one: `ibl-bg` takes its layout from a
# PARAMETER, two calls and one module away from the `let` that holds it — which is how ten of the
# renderer's fifteen bind groups are written, and what a resolver that stops at "is this a name I
# know?" cannot follow. `Decoy::create` and `std::fs::File::create` sit beside it so that a resolver
# matching call sites by bare name fails this family instead of the real tree.

_RES_SHADER = """
struct Camera { view_proj: mat4x4<f32> };
@group(0) @binding(0) var<uniform> cam: Camera;
@group(1) @binding(0) var env: texture_2d<f32>;
@group(1) @binding(1) var env_samp: sampler;
struct Out { @builtin(position) pos: vec4<f32>, @location(0) n: vec3<f32> };
@vertex
fn vs_main(@location(0) p: vec3<f32>) -> Out {
    var o: Out; o.pos = cam.view_proj * vec4<f32>(p, 1.0); o.n = p; return o;
}
@fragment
fn fs_main(inp: Out) -> @location(0) vec4<f32> { return textureSample(env, env_samp, inp.n.xy); }
"""

_RES_RENDERER = """
const S: &str = include_str!("s.wgsl");
struct Decoy;
impl Decoy {
    fn create(a: u32) -> u32 { a }
}
struct Targets;
impl Targets {
    fn create(device: &D, bgl: &wgpu::BindGroupLayout) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("target-bg"),
            layout: bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: cam_buf.as_entire_binding(),
            }],
        })
    }
}
fn build(device: &D) {
    let _targets = Targets::create(&device, &cam_bgl);
    let cam_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("cam-bgl"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    });
    let ibl_bgl = crate::ibl::bind_group_layout(&device);
    let _decoy = Decoy::create(1);
    let _file = std::fs::File::create("out.png");
    let _ibl_bg = crate::ibl::create(&device, &ibl_bgl);
    let cam_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("cam-bg"),
        layout: &cam_bgl,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: cam_buf.as_entire_binding(),
        }],
    });
    let vbl = wgpu::VertexBufferLayout { array_stride: 12, step_mode: m, attributes: &[
        wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 0, shader_location: 0 },
    ]};
    let lay = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None, bind_group_layouts: &[Some(&cam_bgl), Some(&ibl_bgl)], immediate_size: 0 });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor { label: Some("main"),
        layout: Some(&lay),
        vertex: wgpu::VertexState { module: &s, entry_point: Some("vs_main"),
            buffers: std::slice::from_ref(&vbl), compilation_options: d },
        fragment: Some(wgpu::FragmentState { module: &s, entry_point: Some("fs_main"),
            targets: &[], compilation_options: d }), });
}
"""

_RES_IBL = """
pub fn bind_group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("ibl-bgl"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    })
}

pub fn create(device: &wgpu::Device, layout: &wgpu::BindGroupLayout) -> wgpu::BindGroup {
    create_with(device, layout)
}

fn create_with(device: &wgpu::Device, layout: &wgpu::BindGroupLayout) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("ibl-bg"),
        layout,
        entries: &[
            bg_entry(0, wgpu::BindingResource::TextureView(&env_view)),
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&env_samp),
            },
        ],
    })
}

fn bg_entry(
    binding: u32,
    resource: wgpu::BindingResource,
) -> wgpu::BindGroupEntry {
    wgpu::BindGroupEntry { binding, resource }
}
"""


def _self_test_resources() -> bool:
    import tempfile

    ok = True

    def run_case(
        name: str,
        renderer: str = _RES_RENDERER,
        ibl: str = _RES_IBL,
        shader: str = _RES_SHADER,
        want: tuple[str, str, str, str | None] | None = None,
    ) -> None:
        nonlocal ok
        with tempfile.TemporaryDirectory() as tmp:
            src = os.path.join(tmp, "src")
            os.makedirs(src)
            open(os.path.join(src, "s.wgsl"), "w", encoding="utf-8").write(shader)
            open(os.path.join(src, "case.rs"), "w", encoding="utf-8").write(renderer)
            open(os.path.join(src, "ibl.rs"), "w", encoding="utf-8").write(ibl)
            found, _, _, stats = run(tmp)
            if want is None:
                if found or stats["resources"] == 0:
                    ok = False
                    print(
                        f"FAIL  {name}: expected a clean tree with resources compared, got "
                        f"{stats['resources']} compared and {[(f.check, f.message[:70]) for f in found]}"
                    )
                else:
                    print(f"pass  {name} ({stats['resources']} bind group entr(ies) compared, 0 findings)")
                return
            check, severity, text, where_prefix = want
            hit = [
                f
                for f in found
                if f.check == check
                and f.severity == severity
                and text in f.message
                and (where_prefix is None or f.where.startswith(where_prefix))
            ]
            if hit:
                print(f"pass  caught: {name}\n        → {hit[0].where}: {hit[0].message}")
            else:
                ok = False
                print(
                    f"FAIL  missed: {name} (expected a `{check}`/{severity} at {where_prefix} saying "
                    f"{text!r}, got {[(f.check, f.severity, f.where, f.message[:70]) for f in found]})"
                )

    # 0. The reference tree, and the reach assertion beside it. `ibl-bg`'s layout is a parameter
    #    reached through `ibl::create` from another file, with two same-named `create`s in the way —
    #    so a non-zero count here IS the resolver working, not an incidental number.
    run_case("bind groups whose layouts resolve across files audit clean, and are actually compared")

    ibl_at = "src/ibl.rs"

    # 1. The class swap, through the BUILDER — `fn bg_entry(..) -> wgpu::BindGroupEntry {` matches
    #    the same text a struct literal does, and reading the signature as the literal makes every
    #    builder-built entry parse as a missing `binding:`. That mistake cost the `bindings` check a
    #    version; the guard is here too, and this case is what keeps it honest.
    #    wgpu at `create_bind_group`: "Binding 0 entry type mismatch".
    run_case(
        "a sampler bound where the layout declares a texture, through an entry builder",
        ibl=_RES_IBL.replace(
            "bg_entry(0, wgpu::BindingResource::TextureView(&env_view)),",
            "bg_entry(0, wgpu::BindingResource::Sampler(&env_samp)),",
        ),
        want=("resources", "error", 'wgpu: "Binding 0 entry type mismatch"', ibl_at),
    )

    # 2. An index the layout has no slot for.
    run_case(
        "a bind group entry at an index the layout does not declare",
        ibl=_RES_IBL.replace(
            "binding: 1,\n                resource: wgpu::BindingResource::Sampler(&env_samp),",
            "binding: 4,\n                resource: wgpu::BindingResource::Sampler(&env_samp),",
        ),
        want=("resources", "error", "the layout has no slot for", ibl_at),
    )

    # 3. The other direction: a layout entry nothing answers. wgpu counts the entries and refuses.
    run_case(
        "a layout entry no bind group entry answers",
        ibl=_RES_IBL.replace(
            """            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&env_samp),
            },
""",
            "",
        ),
        want=("resources", "error", "nothing is bound at @binding 1", ibl_at),
    )

    # 4. The same index twice — legal Rust, and a bind group wgpu will not build.
    run_case(
        "the same binding index supplied twice in one bind group",
        ibl=_RES_IBL.replace(
            "binding: 1,\n                resource: wgpu::BindingResource::Sampler(&env_samp),",
            "binding: 0,\n                resource: wgpu::BindingResource::Sampler(&env_samp),",
        ),
        want=("resources", "error", "twice", ibl_at),
    )

    # 5. A layout expression the resolver cannot follow. UNRESOLVED, loudly — the resources in that
    #    group are unchecked, and "unchecked" printed as "clean" is this tool's whole subject.
    run_case(
        "a bind group whose layout expression cannot be resolved",
        ibl=_RES_IBL.replace("        layout,\n", "        layout: cache.layout_for(kind),\n"),
        want=("resources", "unresolved", "are UNCHECKED, not clean", ibl_at),
    )

    # 6. A `create_bind_group` in a descriptor shape the parser cannot read. The file still parses,
    #    still has pipelines, still audits clean for everything else — which is precisely how
    #    `mesh_frame_bench` rotted. Counted against the call sites so it cannot be silent.
    run_case(
        "a create_bind_group call the parser cannot read",
        ibl=_RES_IBL.replace("device.create_bind_group(&wgpu::BindGroupDescriptor {", "device.create_bind_group(&descriptor_for(kind) {"),
        want=("coverage", "unresolved", "UNREADABLE to the gate, not clean", ibl_at),
    )

    # 6b. A SECOND caller whose layout argument cannot be read. The first one resolves perfectly, and
    #     "the sites that resolved agree" is the tempting answer — it is also wrong, and it is the
    #     answer the first version of this resolver gave: `make_inst_bg` is called from two impls in
    #     the real renderer and it reported one of the two layouts with no hint that it had dropped
    #     the other. One unreadable call site poisons the whole result, by construction.
    run_case(
        "a helper called from one site that resolves and one that does not",
        renderer=_RES_RENDERER.replace(
            "    let _decoy = Decoy::create(1);",
            "    let _decoy = Decoy::create(1);\n    let _bg2 = crate::ibl::create(&device, registry.layout(2));",
        ),
        want=("resources", "unresolved", "are UNCHECKED, not clean", ibl_at),
    )

    # 6c. And the rejection rule the resolution depends on. `Decoy::create` and `std::fs::File::create`
    #     sit in the fixture precisely so that a resolver matching call sites by bare name reads `1`
    #     and `"out.png"` as bind group layouts. This case asserts the mechanism from the other end:
    #     a decoy whose argument WOULD resolve to the wrong layout, and the answer that must ignore it.
    run_case(
        "a same-named function whose argument is a different bind group layout",
        renderer=_RES_RENDERER.replace(
            "    let _decoy = Decoy::create(1);",
            "    let _decoy = Decoy::create(1);\n    let _other = std::fs::File::create(&cam_bgl);",
        ),
        ibl=_RES_IBL.replace(
            "bg_entry(0, wgpu::BindingResource::TextureView(&env_view)),",
            "bg_entry(0, wgpu::BindingResource::Sampler(&env_samp)),",
        ),
        want=("resources", "error", "bind_group_layout (src/ibl.rs", ibl_at),
    )

    # 7. The check on the check. Bind groups parsed, not one entry compared: the `resources` check
    #    switched off reports the same nothing as the `resources` check agreeing.
    run_case(
        "bind groups parsed with nothing compared against a layout",
        renderer=_RES_RENDERER.replace("let ibl_bgl = crate::ibl::bind_group_layout(&device);", "let ibl_bgl = elsewhere!();")
        .replace("layout: &cam_bgl,\n        entries", "layout: registry.get(0),\n        entries")
        .replace("layout: bgl,\n", "layout: registry.get(1),\n"),
        ibl=_RES_IBL.replace("        layout,\n", "        layout: cache.layout_for(kind),\n"),
        want=("coverage", "error", "the `resources` check is switched off, not passing", None),
    )
    return ok


# ── the `variants` family ─────────────────────────────────────────────────────────────────────────
# `render.rs` does not give wgpu `ssao.wgsl` as written: with MSAA off it swaps the block between two
# marker comments, because one WGSL module cannot hold both `texture_depth_multisampled_2d` and
# `texture_depth_2d`. So one of the two shaders that reach the GPU exists only at run time, and the
# audit had never seen it — proved by drifting that substituted block twice into modules wgpu refuses
# and watching the gate pass both.

_VAR_SHADER = """
struct Camera { view_proj: mat4x4<f32> };
@group(0) @binding(0) var<uniform> cam: Camera;
// >>> DEPTH BINDING BLOCK <<<
@group(0) @binding(1) var depth_tex: texture_depth_multisampled_2d;
fn depth_texel(c: vec2<i32>) -> f32 { return textureLoad(depth_tex, c, 0); }
// >>> END DEPTH BINDING BLOCK <<<
struct Out { @builtin(position) pos: vec4<f32> };
@vertex
fn vs_main(@location(0) p: vec3<f32>) -> Out { var o: Out; o.pos = cam.view_proj * vec4<f32>(p, 1.0); return o; }
@fragment
fn fs_main(inp: Out) -> @location(0) vec4<f32> { return vec4<f32>(depth_texel(vec2<i32>(0, 0))); }
"""

_VAR_RENDERER = '''
const S: &str = include_str!("s.wgsl");
const DEPTH_START: &str = "// >>> DEPTH BINDING BLOCK <<<";
const DEPTH_END: &str = "// >>> END DEPTH BINDING BLOCK <<<";
const SINGLE_SAMPLE: &str = "\\
@group(0) @binding(1) var depth_tex: texture_depth_2d;
fn depth_texel(c: vec2<i32>) -> f32 { return textureLoad(depth_tex, c, 0); }
";
fn build(device: &D) {
    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("bgl"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: samples > 1,
                },
                count: None,
            },
        ],
    });
    let vbl = wgpu::VertexBufferLayout { array_stride: 12, step_mode: m, attributes: &[
        wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 0, shader_location: 0 },
    ]};
    let lay = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None, bind_group_layouts: &[Some(&bgl)], immediate_size: 0 });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor { label: Some("ssao"),
        layout: Some(&lay),
        vertex: wgpu::VertexState { module: &s, entry_point: Some("vs_main"),
            buffers: std::slice::from_ref(&vbl), compilation_options: d },
        fragment: Some(wgpu::FragmentState { module: &s, entry_point: Some("fs_main"),
            targets: &[], compilation_options: d }), });
}
'''


def _self_test_variants() -> bool:
    import tempfile

    ok = True

    def run_case(
        name: str,
        renderer: str = _VAR_RENDERER,
        shader: str = _VAR_SHADER,
        want: tuple[str, str, str] | None = None,
    ) -> None:
        nonlocal ok
        with tempfile.TemporaryDirectory() as tmp:
            src = os.path.join(tmp, "src")
            os.makedirs(src)
            open(os.path.join(src, "s.wgsl"), "w", encoding="utf-8").write(shader)
            open(os.path.join(src, "case.rs"), "w", encoding="utf-8").write(renderer)
            found, _, _, stats = run(tmp)
            if want is None:
                if found or stats["variants"] == 0:
                    ok = False
                    print(
                        f"FAIL  {name}: expected a clean tree with a variant rebuilt, got "
                        f"{stats['variants']} variant(s) and {[(f.check, f.message[:70]) for f in found]}"
                    )
                else:
                    print(f"pass  {name} ({stats['variants']} run-time variant(s) rebuilt, 0 findings)")
                return
            check, severity, text = want
            hit = [f for f in found if f.check == check and f.severity == severity and text in f.message]
            if hit:
                print(f"pass  caught: {name}\n        → {hit[0].where}: {hit[0].message}")
            else:
                ok = False
                print(
                    f"FAIL  missed: {name} (expected a `{check}`/{severity} saying {text!r}, got "
                    f"{[(f.check, f.severity, f.where, f.message[:70]) for f in found]})"
                )

    # 0. Both modules agree with the layout, and the variant was actually built. The count is part of
    #    the assertion for the usual reason: a tree with no variant rebuilt is clean too.
    run_case("a substituted variant that agrees with its layout is rebuilt and audits clean")

    # 1. The drift that used to be invisible: only the SUBSTITUTED block moves. The file at rest is
    #    still perfect, and the module the host compiles with MSAA off is one wgpu refuses.
    run_case(
        "the substituted block moved to a binding index the layout does not declare",
        renderer=_VAR_RENDERER.replace(
            "@group(0) @binding(1) var depth_tex: texture_depth_2d;",
            "@group(0) @binding(9) var depth_tex: texture_depth_2d;",
        ),
        want=("bindings", "error", "in the run-time variant"),
    )

    # 2. The same block, right index, wrong type — and pinned to the TYPE message, so a case that
    #    passes because some other finding fired cannot stand in for this one. The expectation names
    #    what the SHADER needs (`texture:float:D2`), not what the layout says: the first version of
    #    this case asserted the layout's side and passed nothing, which is a case that would have
    #    stayed red however correct the gate was.
    run_case(
        "the substituted block declaring a sampled float texture where the layout says depth",
        renderer=_VAR_RENDERER.replace(
            "var depth_tex: texture_depth_2d;", "var depth_tex: texture_2d<f32>;"
        ),
        want=("bindings", "error", "which needs `texture:float:D2`"),
    )

    # 3. Markers with no substitution the audit can build. The shader that reaches the GPU is then a
    #    module NO file states, and the only honest answer is to say so.
    run_case(
        "substitution markers whose replacement const the audit cannot find",
        renderer=_VAR_RENDERER.replace('const SINGLE_SAMPLE: &str = "\\', 'const SINGLE_SAMPLE_DISABLED: &str = "\\').replace(
            "@group(0) @binding(1) var depth_tex: texture_depth_2d;", "// no wgsl here"
        ),
        want=("entries", "unresolved", "the shader the host actually compiles is UNAUDITED"),
    )

    # 4. A substitution that does not parse as WGSL. The host compiles this at start-up; a variant
    #    that cannot be read must not fall back to reading the file at rest and calling it clean.
    run_case(
        "a substituted block that is not valid WGSL",
        renderer=_VAR_RENDERER.replace(
            "fn depth_texel(c: vec2<i32>) -> f32 { return textureLoad(depth_tex, c, 0); }",
            "fn depth_texel(c: vec2<i32>) -> f32 { return textureLoad(depth_tex, c, 0);",
        ),
        want=("entries", "error", "does not parse as WGSL"),
    )

    # 4b. The decoder, asserted on its value rather than on a verdict. `const X: &str = "\\` + newline
    #     is how `render.rs` writes the substituted block, and a decoder that leaves the continuation
    #     in place still produces a variant that PARSES and still AGREES — so no verdict-shaped case
    #     can see the difference, and the substituted source would silently be one character wrong
    #     for every later reader. Pinned to the text.
    got = rustgpu.string_consts(_VAR_RENDERER).get("SINGLE_SAMPLE", "")
    if got.startswith("@group(0) @binding(1) var depth_tex") and "\\" not in got:
        print("pass  a `\\`-continued string const decodes to exactly the WGSL it holds")
    else:
        ok = False
        print(f"FAIL  the string const decoder produced {got[:60]!r}")

    # 5. The variant pass must not INVENT findings. Every drift above is in the substituted block; if
    #    the variant pass reported the base module's own bindings a second time, case 0 would be the
    #    only thing standing between that and a wall of duplicate noise — so pin it directly: a
    #    defect in the UNSUBSTITUTED part of the shader is reported once, unlabelled, by the base
    #    pass, and not again by the variant.
    with tempfile.TemporaryDirectory() as tmp:
        src = os.path.join(tmp, "src")
        os.makedirs(src)
        open(os.path.join(src, "s.wgsl"), "w", encoding="utf-8").write(
            _VAR_SHADER.replace("@group(0) @binding(0) var<uniform> cam", "@group(0) @binding(5) var<uniform> cam")
        )
        open(os.path.join(src, "case.rs"), "w", encoding="utf-8").write(_VAR_RENDERER)
        found, _, _, _ = run(tmp)
        shared = [f for f in found if "@binding(5)" in f.message]
        labelled = [f for f in shared if "run-time variant" in f.message]
        if len(shared) == 1 and not labelled:
            print("pass  a defect outside the substituted block is reported once, by the base pass")
        else:
            ok = False
            print(
                f"FAIL  the variant pass duplicated a base finding: {[f.message[:80] for f in shared]}"
            )
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
            found, _, _, _ = run(tmp)
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
    ok = _self_test_bindings() and ok
    ok = _self_test_resources() and ok
    ok = _self_test_variants() and ok

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
