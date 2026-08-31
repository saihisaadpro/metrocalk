"""The Rust half of the contract: what the host says a pipeline is.

`render.rs` and every offscreen example describe the same pipelines a second time, in Rust — the
vertex buffer layout, the two entry points, the bind group layouts, and the `#[repr(C)]` structs
whose bytes the shader reads. wgpu compares the two descriptions at pipeline creation and refuses
the pipeline when they disagree. This module recovers the Rust description so the comparison can
happen at rest instead.

It reads declarations, not Rust. Three construction shapes are understood, because those are the
three this codebase uses:

  1. a literal `wgpu::RenderPipelineDescriptor { .. }`;
  2. a helper `fn` whose body builds one from its parameters, resolved per call site;
  3. a `let f = |..| { .. }` closure, likewise.

The same three shapes appear one level down, in the `wgpu::BindGroupLayoutDescriptor` whose entries
answer the shader's `@binding`s — plus a fourth, a layout built in another module and bound by the
`let` it lands in (`let ibl_bgl = crate::ibl::bind_group_layout(&device);`). All four are read,
because the group they build is one the pipeline layout already declares, and a binding inside an
already-declared group is precisely what the `@group`-level check could not see.

Anything else is recorded as UNRESOLVED and reported. The audit never treats "I could not read it"
as "it is fine" — that is the failure mode being fixed.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field

# ── wgpu vertex formats, as the shader sees them ──────────────────────────────────────────────────
# wgpu requires the shader's input type to match the format's scalar KIND, and the format to supply
# at least as many components as the shader reads (extra components are dropped, missing ones are
# an error). Sizes are needed for the stride/offset check.

_FORMATS: dict[str, tuple[str, int, int]] = {}
for _n, _sz in (("8", 1), ("16", 2), ("32", 4)):
    for _kind, _wgsl in (("Float", "f32"), ("Sint", "i32"), ("Uint", "u32")):
        for _c in (1, 2, 3, 4):
            _suffix = "" if _c == 1 else f"x{_c}"
            if _n != "32" and _c in (1, 3):
                continue  # wgpu has no 8/16-bit 1- or 3-component vertex formats
            _FORMATS[f"{_kind}{_n}{_suffix}"] = (_wgsl, _c, _sz * _c)
for _n in ("8", "16"):
    for _kind in ("Unorm", "Snorm"):
        for _c in (2, 4):
            _FORMATS[f"{_kind}{_n}x{_c}"] = ("f32", _c, (1 if _n == "8" else 2) * _c)
_FORMATS["Float16x2"] = ("f32", 2, 4)
_FORMATS["Float16x4"] = ("f32", 4, 8)


def format_info(name: str) -> tuple[str, int, int] | None:
    """`Float32x3` → (scalar wgsl type, component count, bytes)."""
    return _FORMATS.get(name)


# ── declarations ──────────────────────────────────────────────────────────────────────────────────


@dataclass
class Attribute:
    format: str
    offset: int
    location: int
    line: int


@dataclass
class VertexLayout:
    name: str
    stride: str
    attributes: list[Attribute]
    line: int


@dataclass
class PipelineLayout:
    name: str
    groups: list[str]  # the bind-group-layout expression per index, in order
    line: int


@dataclass
class Pipeline:
    label: str
    vertex_entry: str | None
    fragment_entry: str | None
    buffers: str | None  # the expression, e.g. "from_ref(&mesh_vbl)" or "&[]"
    layout: str | None  # the pipeline-layout identifier
    line: int
    topology: str | None = None  # TriangleList / LineList / …
    unresolved: list[str] = field(default_factory=list)


@dataclass
class BindingEntry:
    """One `wgpu::BindGroupLayoutEntry`, reduced to the four things the shader also states.

    `kind` is canonical rather than verbatim, because the two sides spell the same fact
    differently: WGSL says `texture_depth_2d`, Rust says `TextureSampleType::Depth` +
    `TextureViewDimension::D2`. Sub-attributes WGSL does **not** encode (`filterable`,
    `has_dynamic_offset`) are deliberately absent — comparing them would be inventing a
    disagreement out of a fact only one side states.
    """

    binding: int | None
    visibility: frozenset[str]  # {"VERTEX", "FRAGMENT", "COMPUTE"}; empty when unreadable
    kind: str  # canonical, e.g. "buffer:uniform" / "texture:depth:D2" / "sampler:comparison"
    min_binding_size: int | str | None  # int = literal · str = an expression, unevaluated · None = absent
    multisampled: str | None  # the raw expression; see audit.py for why it is not compared
    line: int
    unresolved: list[str] = field(default_factory=list)


@dataclass
class BindGroupLayout:
    name: str
    label: str | None
    entries: list[BindingEntry]
    line: int
    unresolved: list[str] = field(default_factory=list)


@dataclass
class BindGroupUse:
    """One `wgpu::BindGroupEntry`, reduced to the two things the LAYOUT also states.

    A bind group is the third statement of the same contract, and the one wgpu checks last —
    `create_bind_group` rejects a `TextureView` where the layout says `Buffer`, an index the layout
    does not declare, and a group that supplies fewer entries than the layout has. `kind` is the
    RESOURCE CLASS only (`buffer` / `texture` / `sampler`), because that is the whole of what a
    `BindingResource` variant states: `buf.as_entire_binding()` says "a buffer" and is silent about
    uniform-vs-storage, which wgpu resolves against the buffer's USAGE FLAGS at run time — a fact
    neither of these two files holds.
    """

    binding: int | None
    kind: str  # "buffer" | "texture" | "sampler" | "acceleration-structure"; "" when unreadable
    line: int
    unresolved: list[str] = field(default_factory=list)


@dataclass
class BindGroup:
    label: str | None
    layout_expr: str  # verbatim; resolved to layout name(s) by `LayoutResolver`
    entries: list[BindGroupUse]
    line: int
    at: int  # index of the descriptor's opening brace, for the resolver's scope walk
    unresolved: list[str] = field(default_factory=list)


@dataclass
class ReprCStruct:
    name: str
    fields: list[tuple[str, str]]
    line: int


@dataclass
class RustFile:
    path: str
    src: str
    vertex_layouts: dict[str, VertexLayout]
    pipeline_layouts: dict[str, PipelineLayout]
    pipelines: list[Pipeline]
    structs: dict[str, ReprCStruct]
    shader_sources: list[str]  # `include_str!("...")` paths
    bind_group_layouts: dict[str, BindGroupLayout] = field(default_factory=dict)
    # `let ibl_bgl = crate::ibl::bind_group_layout(&device);` — the layout this pipeline binds at
    # group 3 is built in another file. Without this the group-3 entries are simply not there, and
    # "not there" is the shape every blind spot in this tool has had.
    bgl_aliases: dict[str, str] = field(default_factory=dict)
    bind_groups: list[BindGroup] = field(default_factory=list)


# ── helpers ───────────────────────────────────────────────────────────────────────────────────────


def _line_of(src: str, idx: int) -> int:
    return src.count("\n", 0, idx) + 1


def _match_brace(src: str, open_at: int, opener: str = "{", closer: str = "}") -> int:
    depth = 0
    i = open_at
    while i < len(src):
        c = src[i]
        if c == '"':  # skip string literals, which contain braces in labels
            i += 1
            while i < len(src) and src[i] != '"':
                i += 2 if src[i] == "\\" else 1
        elif c == opener:
            depth += 1
        elif c == closer:
            depth -= 1
            if depth == 0:
                return i + 1
        i += 1
    raise ValueError("unbalanced")


def _top_level_spans(text: str, sep: str = ",") -> list[tuple[int, int]]:
    """(start, end) of each `sep`-separated part at bracket depth zero, as indices into `text`.

    `<`/`>` are brackets in `Vec<A, B>`, arrows in `=>` / `->`, and comparison operators in
    `multisampled: samples > 1`. Counting any of the last two as a closer is not a cosmetic bug: it
    drives the depth negative, so the very next comma reads as top level and the field is silently
    truncated mid-expression. That is how a `match` arm selecting a fragment entry point went unread
    — and, later, how the whole `entries:` array of `ssao-input-bgl` did, because its depth binding
    is `multisampled: samples > 1` and the `>` there closes nothing.
    So `<` opens only where a generic can begin (after an identifier character, `:` or `>`), and `>`
    closes only when such a `<` is actually open.
    """
    spans: list[tuple[int, int]] = []
    depth = angle = 0
    start, i, prev = 0, 0, ""
    while i < len(text):
        ch = text[i]
        if ch == '"':
            i += 1
            while i < len(text) and text[i] != '"':
                i += 2 if text[i] == "\\" else 1
            i += 1
            prev = '"'
            continue
        if ch == "<" and (prev.isalnum() or prev in "_:>"):
            angle += 1
            depth += 1
        elif ch == ">" and angle > 0:
            angle -= 1
            depth -= 1
        elif ch in "([{":
            depth += 1
        elif ch in ")]}":
            depth -= 1
        elif ch == sep and depth == 0:
            spans.append((start, i))
            start = i + 1
        if not ch.isspace():
            prev = ch
        i += 1
    spans.append((start, len(text)))
    out = []
    for s, e in spans:
        part = text[s:e]
        lead = len(part) - len(part.lstrip())
        stripped = part.strip()
        if stripped:
            out.append((s + lead, s + lead + len(stripped)))
    return out


def _split_top_level(text: str, sep: str = ",") -> list[str]:
    return [text[s:e] for s, e in _top_level_spans(text, sep)]


def _field(body: str, name: str) -> str | None:
    """Read `name: <expr>` out of a struct-literal body, at top level only.

    Field-init shorthand (`buffers,`) counts: it is the same binding written shorter, and reading
    it as absent is how a helper's vertex buffers became invisible to this audit.
    """
    for part in _split_top_level(body):
        if re.match(rf"^{re.escape(name)}\s*:", part):
            return part.split(":", 1)[1].strip()
        if re.fullmatch(re.escape(name), part.strip()):
            return name
    return None


def strip_comments(src: str) -> str:
    out, i, n = [], 0, len(src)
    while i < n:
        c = src[i]
        if c == '"':
            out.append(c)
            i += 1
            while i < n and src[i] != '"':
                if src[i] == "\\":
                    out.append(src[i])
                    i += 1
                if i < n:
                    out.append(src[i])
                    i += 1
            if i < n:
                out.append(src[i])
                i += 1
            continue
        if src.startswith("//", i):
            j = src.find("\n", i)
            j = n if j < 0 else j
            out.append(" " * (j - i))
            i = j
            continue
        if src.startswith("/*", i):
            j = src.find("*/", i)
            j = n if j < 0 else j + 2
            out.append(re.sub(r"[^\n]", " ", src[i:j]))
            i = j
            continue
        out.append(c)
        i += 1
    return "".join(out)


# ── parsing ───────────────────────────────────────────────────────────────────────────────────────

_VBL = re.compile(r"(?:let\s+(\w+)\s*(?::[^=]+)?=\s*)?wgpu::VertexBufferLayout\s*\{")
_ATTR = re.compile(
    r"wgpu::VertexAttribute\s*\{(?P<body>[^{}]*)\}", re.S
)
_PIPELINE_LAYOUT = re.compile(
    r"let\s+(\w+)\s*=\s*device\.create_pipeline_layout\(\s*&wgpu::PipelineLayoutDescriptor\s*\{"
)
_RPD = re.compile(r"wgpu::RenderPipelineDescriptor\s*\{")
_REPR_C = re.compile(r"#\[repr\(C\)\][\s\S]{0,200}?\bstruct\s+(\w+)\s*\{")
_INCLUDE_STR = re.compile(r'include_str!\(\s*"([^"]+)"\s*\)')


def _parse_attributes(body: str, src: str, base: int) -> list[Attribute]:
    out = []
    for m in _ATTR.finditer(body):
        b = m.group("body")
        fmt = _field(b, "format") or ""
        off = _field(b, "offset") or "0"
        loc = _field(b, "shader_location") or "-1"
        fmt = fmt.replace("wgpu::VertexFormat::", "").strip()
        try:
            out.append(
                Attribute(
                    format=fmt,
                    offset=int(off, 0),
                    location=int(loc, 0),
                    line=_line_of(src, base + m.start()),
                )
            )
        except ValueError:
            out.append(Attribute(format=fmt, offset=-1, location=-1, line=_line_of(src, base + m.start())))
    return out


def parse_rust(path: str, raw: str) -> RustFile:
    src = strip_comments(raw)

    vertex_layouts: dict[str, VertexLayout] = {}
    anon = 0
    for m in _VBL.finditer(src):
        end = _match_brace(src, m.end() - 1)
        body = src[m.end() : end - 1]
        name = m.group(1)
        if not name:
            anon += 1
            name = f"<anonymous #{anon}>"
        vertex_layouts[name] = VertexLayout(
            name=name,
            stride=(_field(body, "array_stride") or "?").strip(),
            attributes=_parse_attributes(body, src, m.end()),
            line=_line_of(src, m.start()),
        )

    pipeline_layouts: dict[str, PipelineLayout] = {}
    for m in _PIPELINE_LAYOUT.finditer(src):
        end = _match_brace(src, m.end() - 1)
        body = src[m.end() : end - 1]
        groups_expr = _field(body, "bind_group_layouts") or "&[]"
        inner = groups_expr.strip()
        if inner.startswith("&["):
            inner = inner[2 : inner.rfind("]")]
        pipeline_layouts[m.group(1)] = PipelineLayout(
            name=m.group(1),
            groups=[_ident(g) for g in _split_top_level(inner)],
            line=_line_of(src, m.start()),
        )

    structs: dict[str, ReprCStruct] = {}
    for m in _REPR_C.finditer(src):
        end = _match_brace(src, m.end() - 1)
        body = src[m.end() : end - 1]
        fields = []
        for part in _split_top_level(body):
            if ":" in part and not part.startswith("//"):
                fname, fty = part.split(":", 1)
                fields.append((fname.strip(), fty.strip()))
        structs[m.group(1)] = ReprCStruct(m.group(1), fields, _line_of(src, m.start()))

    pipelines = _parse_pipelines(src)
    shaders = [m.group(1) for m in _INCLUDE_STR.finditer(src) if m.group(1).endswith(".wgsl")]
    bgls, aliases = _parse_bind_group_layouts(src)
    return RustFile(
        path,
        src,
        vertex_layouts,
        pipeline_layouts,
        pipelines,
        structs,
        shaders,
        bind_group_layouts=bgls,
        bgl_aliases=aliases,
        bind_groups=_parse_bind_groups(src),
    )


def _ident(expr: str) -> str:
    """`Some(&cam_bgl)` → `cam_bgl`; `&mesh_layout` → `mesh_layout`."""
    e = expr.strip()
    m = re.search(r"&\s*([A-Za-z_]\w*)", e)
    if m:
        return m.group(1)
    return re.sub(r"^[&*]+", "", e)


def _string(expr: str) -> str | None:
    m = re.fullmatch(r'Some\(\s*"([^"]*)"\s*\)|"([^"]*)"', expr.strip())
    if not m:
        return None
    return m.group(1) if m.group(1) is not None else m.group(2)


@dataclass
class _Template:
    """A pipeline descriptor written in terms of names — a helper's parameters, or plain idents."""

    label: str | None
    vertex_expr: str | None
    fragment_expr: str | None
    buffers_expr: str | None
    layout_expr: str | None
    topology_expr: str | None
    line: int


def _descriptor(src: str, at: int) -> _Template:
    end = _match_brace(src, at)
    body = src[at + 1 : end - 1]
    vertex = _field(body, "vertex") or ""
    frag = _field(body, "fragment") or ""
    prim = _field(body, "primitive") or ""
    vbody = vertex[vertex.find("{") + 1 : vertex.rfind("}")] if "{" in vertex else ""
    fbody = frag[frag.find("{") + 1 : frag.rfind("}")] if "{" in frag else ""
    pbody = prim[prim.find("{") + 1 : prim.rfind("}")] if "{" in prim else ""
    return _Template(
        label=_field(body, "label"),
        vertex_expr=_field(vbody, "entry_point"),
        fragment_expr=_field(fbody, "entry_point"),
        buffers_expr=_field(vbody, "buffers"),
        layout_expr=_field(body, "layout"),
        topology_expr=_field(pbody, "topology"),
        line=_line_of(src, at),
    )


_FN_DEF = re.compile(r"\bfn\s+(\w+)\s*(?:<[^>]*>)?\s*\(")
_CLOSURE = re.compile(r"let\s+(\w+)\s*=\s*\|")


def _parse_pipelines(src: str) -> list[Pipeline]:
    out: list[Pipeline] = []

    # Every descriptor literal, with the enclosing helper (if any) resolved by span.
    helpers: dict[str, tuple[list[str], _Template]] = {}
    helper_spans: list[tuple[int, int, str]] = []

    for m in _FN_DEF.finditer(src):
        open_paren = m.end() - 1
        try:
            close = _match_brace(src, open_paren, "(", ")")
        except ValueError:
            continue
        params = [_param_name(p) for p in _split_top_level(src[open_paren + 1 : close - 1])]
        brace = src.find("{", close)
        if brace < 0:
            continue
        try:
            body_end = _match_brace(src, brace)
        except ValueError:
            continue
        helper_spans.append((brace, body_end, m.group(1)))
        helpers.setdefault(m.group(1), (params, None))  # type: ignore[arg-type]

    for m in _CLOSURE.finditer(src):
        bar = m.end() - 1
        close = src.find("|", bar + 1)
        params = [_param_name(p) for p in _split_top_level(src[bar + 1 : close])]
        brace = src.find("{", close)
        if brace < 0:
            continue
        try:
            body_end = _match_brace(src, brace)
        except ValueError:
            continue
        helper_spans.append((brace, body_end, m.group(1)))
        helpers.setdefault(m.group(1), (params, None))  # type: ignore[arg-type]

    for m in _RPD.finditer(src):
        at = m.end() - 1
        tpl = _descriptor(src, at)
        # innermost enclosing helper
        owner = None
        best = -1
        for s, e, name in helper_spans:
            if s < at < e and s > best:
                best, owner = s, name
        if owner is not None and _template_is_parametric(tpl, helpers[owner][0]):
            params, _ = helpers[owner]
            helpers[owner] = (params, tpl)
        else:
            out.append(_concrete(tpl, {}))

    # resolve call sites of parametric helpers
    for name, (params, tpl) in helpers.items():
        if tpl is None:
            continue
        for call in re.finditer(rf"\b{re.escape(name)}\s*\(", src):
            open_paren = call.end() - 1
            # skip the definition itself
            if any(s < open_paren < e and n == name for s, e, n in helper_spans):
                # a recursive/self call inside its own body — ignore
                pass
            try:
                close = _match_brace(src, open_paren, "(", ")")
            except ValueError:
                continue
            if src[max(0, call.start() - 3) : call.start()].strip().endswith("fn"):
                continue
            args = _split_top_level(src[open_paren + 1 : close - 1])
            if len(args) != len(params):
                continue
            env = dict(zip(params, args))
            p = _concrete(tpl, env)
            p.line = _line_of(src, call.start())
            out.append(p)
    return out


def _param_name(p: str) -> str:
    p = p.strip()
    if ":" in p:
        p = p.split(":", 1)[0]
    return re.sub(r"^(mut|ref)\s+", "", p).strip()


# ── bind group layouts ────────────────────────────────────────────────────────────────────────────
# The `groups` check is `@group`-level: it asks whether the pipeline layout declares enough bind
# groups, and stops there. A new `@binding` inside a group the layout already declares is below its
# resolution — which is how `@group(3) @binding(6)` (the SH irradiance uniform) was added, checked by
# hand, and passed unexamined. wgpu does reject a drifted binding, but only at pipeline creation, on
# a GPU host, in a build that got that far: the exact cost this tool exists to remove.
#
# Four construction shapes are read, because those are the four in use:
#   1. `entries: &[wgpu::BindGroupLayoutEntry { .. }]`                       — a literal
#   2. `entries: &[samp_entry]`                                             — a `let`-bound literal
#   3. `entries: &[bgl_entry(0, VIS, TY)]` / `&[tex(0), samp(1)]`           — an fn or closure builder
#   4. `let ibl_bgl = crate::ibl::bind_group_layout(&device);`              — built in another file
# A fifth must be taught to this module. It will be REPORTED, not skipped.

_BGL_DESC = re.compile(r"create_bind_group_layout\(\s*&\s*wgpu::BindGroupLayoutDescriptor\s*\{")
_BGL_ENTRY_LIT = re.compile(r"wgpu::BindGroupLayoutEntry\s*\{")
_NAMED_ENTRY = re.compile(r"let\s+(\w+)\s*(?::[^=|]+)?=\s*wgpu::BindGroupLayoutEntry\s*\{")
_LET_BEFORE = re.compile(r"let\s+(\w+)\s*(?::[^=]+)?=\s*[^;{}]*$")
_LET_CALL = re.compile(r"let\s+(\w+)\s*(?::[^=]+)?=\s*([A-Za-z_][\w]*(?:\s*::\s*[A-Za-z_]\w*)*)\s*\(")
_UINT = re.compile(r"^\d+$")


@dataclass
class _Callable:
    start: int  # the body's opening brace
    end: int
    params: list[str]
    name: str
    ret: str  # the text between the parameter list and the body brace, e.g. "-> wgpu::BindGroupLayout"


def _callable_spans(src: str) -> list[_Callable]:
    """Every `fn` and `let name = |..|`, with its parameters and return-type text.

    The body start is the first `{` after the parameter list — for a closure whose body is a bare
    struct literal (`|binding| wgpu::BindGroupLayoutEntry { .. }`) that brace IS the literal, so
    ownership is tested with `<=` rather than `<`.
    """
    out: list[_Callable] = []
    for m in _FN_DEF.finditer(src):
        try:
            close = _match_brace(src, m.end() - 1, "(", ")")
        except ValueError:
            continue
        params = [_param_name(p) for p in _split_top_level(src[m.end() : close - 1])]
        brace = src.find("{", close)
        if brace < 0:
            continue
        try:
            out.append(_Callable(brace, _match_brace(src, brace), params, m.group(1), src[close:brace]))
        except ValueError:
            continue
    for m in _CLOSURE.finditer(src):
        bar = m.end() - 1
        close = src.find("|", bar + 1)
        if close < 0:
            continue
        params = [_param_name(p) for p in _split_top_level(src[bar + 1 : close])]
        brace = src.find("{", close)
        if brace < 0:
            continue
        try:
            out.append(_Callable(brace, _match_brace(src, brace), params, m.group(1), src[close + 1 : brace]))
        except ValueError:
            continue
    return out


def _innermost(spans: list[_Callable], at: int, want_ret: str | None = None) -> _Callable | None:
    best: _Callable | None = None
    for c in spans:
        if c.start <= at < c.end and (best is None or c.start > best.start):
            if want_ret is None or want_ret in c.ret:
                best = c
    return best


def _norm_visibility(expr: str | None, env: dict[str, str]) -> frozenset[str]:
    """`ShaderStages::VERTEX_FRAGMENT` → {VERTEX, FRAGMENT}. Unreadable → the empty set."""
    if expr is None:
        return frozenset()
    e = _resolve_ident(expr, env)
    stages: set[str] = set()
    for tok in re.findall(r"ShaderStages\s*::\s*(\w+)", e):
        if tok == "VERTEX_FRAGMENT":
            stages |= {"VERTEX", "FRAGMENT"}
        elif tok in ("VERTEX", "FRAGMENT", "COMPUTE"):
            stages.add(tok)
        elif tok in ("all", "NONE"):
            stages |= {"VERTEX", "FRAGMENT", "COMPUTE"} if tok == "all" else set()
    return frozenset(stages)


def _resolve_ident(expr: str, env: dict[str, str]) -> str:
    e = expr.strip().rstrip(",")
    key = e.lstrip("&").strip()
    if key in env:
        return env[key].strip().rstrip(",")
    return e


def _binding_kind(ty_expr: str | None, env: dict[str, str]) -> tuple[str, int | None, str | None, list[str]]:
    """`BindingType::…` → (canonical kind, min_binding_size, multisampled expr, problems)."""
    if ty_expr is None:
        return "", None, None, ["entry has no `ty:` field"]
    e = _resolve_ident(ty_expr, env)
    m = re.search(r"BindingType\s*::\s*(\w+)", e)
    if not m:
        return "", None, None, [f"unreadable binding type {e[:60]!r}"]
    family = m.group(1)
    if family == "Buffer":
        body = e[e.find("{") + 1 : e.rfind("}")] if "{" in e else ""
        raw_ty = _resolve_ident(_field(body, "ty") or "", env)
        mbs = _min_binding_size(_field(body, "min_binding_size"), env)
        if "Uniform" in raw_ty:
            return "buffer:uniform", mbs, None, []
        if "Storage" in raw_ty:
            ro = _field(raw_ty[raw_ty.find("{") + 1 : raw_ty.rfind("}")] if "{" in raw_ty else "", "read_only")
            if ro is None:
                return "", mbs, None, [f"unreadable storage access in {raw_ty[:60]!r}"]
            return ("buffer:storage-read" if ro.strip().rstrip(",") == "true" else "buffer:storage-rw"), mbs, None, []
        return "", mbs, None, [f"unreadable buffer binding type {raw_ty[:60]!r}"]
    if family == "Sampler":
        inner = _resolve_ident(e[e.find("(") + 1 : e.rfind(")")] if "(" in e else "", env)
        m2 = re.search(r"SamplerBindingType\s*::\s*(\w+)", inner)
        if not m2:
            return "", None, None, [f"unreadable sampler binding type {inner[:60]!r}"]
        return f"sampler:{m2.group(1).lower()}", None, None, []
    if family in ("Texture", "StorageTexture"):
        body = e[e.find("{") + 1 : e.rfind("}")] if "{" in e else ""
        dim = _resolve_ident(_field(body, "view_dimension") or "", env)
        dm = re.search(r"TextureViewDimension\s*::\s*(\w+)", dim)
        if not dm:
            return "", None, None, [f"unreadable view_dimension {dim[:60]!r}"]
        if family == "StorageTexture":
            acc = re.search(r"StorageTextureAccess\s*::\s*(\w+)", _resolve_ident(_field(body, "access") or "", env))
            if not acc:
                return "", None, None, ["unreadable storage-texture access"]
            return f"storage-texture:{acc.group(1).lower()}:{dm.group(1)}", None, None, []
        st = _resolve_ident(_field(body, "sample_type") or "", env)
        sm = re.search(r"TextureSampleType\s*::\s*(\w+)", st)
        if not sm:
            return "", None, None, [f"unreadable sample_type {st[:60]!r}"]
        # `filterable` and `multisampled` are carried but not folded into the kind: WGSL's type name
        # states neither, so a comparison would be manufacturing a disagreement from a fact only the
        # Rust side has. `multisampled` is returned so the audit can say what it did not compare.
        return f"texture:{sm.group(1).lower()}:{dm.group(1)}", None, _field(body, "multisampled"), []
    return "", None, None, [f"binding type `{family}` is not one this audit knows"]


def _min_binding_size(expr: str | None, env: dict[str, str]) -> int | str | None:
    """`None` when the field is absent or `None`, an `int` when it is a literal, else the expression.

    The argument has to be read out of the `new(..)` call, not searched for anywhere in the text:
    the first version of this ran `\\d+` over the whole expression and answered **64** for
    `std::num::NonZeroU64::new(128)` — the type's own name. It still failed the mutation it was
    written for, which is exactly how a gate learns to be confidently wrong: the verdict was right
    and the number in it was not.
    """
    if expr is None:
        return None
    e = _resolve_ident(expr, env).strip()
    if e == "None":
        return None
    m = re.search(r"\bnew\s*\(", e)
    if m:
        try:
            inner = e[m.end() : _match_brace(e, m.end() - 1, "(", ")") - 1]
        except ValueError:
            inner = e[m.end() :]
    else:
        inner = e
    lit = re.fullmatch(r"\s*(\d[\d_]*)\s*(?:u\d+)?\s*", inner)
    if lit:
        return int(lit.group(1).replace("_", ""))
    return inner.strip() or e


def _parse_entry_literal(src: str, brace_at: int, env: dict[str, str]) -> BindingEntry:
    body = src[brace_at + 1 : _match_brace(src, brace_at) - 1]
    problems: list[str] = []
    raw_binding = _field(body, "binding")
    binding: int | None = None
    if raw_binding is None:
        problems.append("entry has no `binding:` field")
    else:
        b = _resolve_ident(raw_binding, env)
        if _UINT.match(b.strip()):
            binding = int(b.strip())
        else:
            problems.append(f"binding index {b[:40]!r} is not a literal")
    vis = _norm_visibility(_field(body, "visibility"), env)
    if not vis:
        problems.append(f"unreadable visibility {(_field(body, 'visibility') or '<missing>')[:40]!r}")
    kind, mbs, ms, kind_problems = _binding_kind(_field(body, "ty"), env)
    problems += kind_problems
    return BindingEntry(
        binding=binding,
        visibility=vis,
        kind=kind,
        min_binding_size=mbs,
        multisampled=ms,
        line=_line_of(src, brace_at),
        unresolved=problems,
    )


def _entry_literal_sites(src: str) -> list[int]:
    """The `{` of every `wgpu::BindGroupLayoutEntry { .. }` **value**.

    `fn bgl_entry(..) -> wgpu::BindGroupLayoutEntry {` matches the same text and is a return type
    followed by a function body, not a literal. Reading it as one is not a near miss: the "entry"
    then has no `binding:`, no `visibility:` and no `ty:` at its top level, so every builder-built
    entry in the renderer parsed as three unresolved fields — a gate that fails for a reason that
    has nothing to do with the code it is auditing.
    """
    out = []
    for m in _BGL_ENTRY_LIT.finditer(src):
        if src[: m.start()].rstrip().endswith("->"):
            continue
        out.append(m.end() - 1)
    return out


def _parse_bind_group_layouts(src: str) -> tuple[dict[str, BindGroupLayout], dict[str, str]]:
    spans = _callable_spans(src)

    # Named entry values: `let samp_entry = wgpu::BindGroupLayoutEntry { .. };`
    named: dict[str, int] = {m.group(1): m.end() - 1 for m in _NAMED_ENTRY.finditer(src)}

    # Builders: an entry literal whose innermost enclosing fn/closure takes parameters it uses.
    builders: dict[str, tuple[list[str], int]] = {}
    for at in _entry_literal_sites(src):
        owner = _innermost(spans, at)
        if owner is None or not owner.params:
            continue
        body = src[at : _match_brace(src, at)]
        if any(re.search(rf"\b{re.escape(p)}\b", body) for p in owner.params if p):
            builders.setdefault(owner.name, (owner.params, at))

    bgls: dict[str, BindGroupLayout] = {}
    for m in _BGL_DESC.finditer(src):
        at = m.end() - 1
        body = src[at + 1 : _match_brace(src, at) - 1]
        line = _line_of(src, m.start())

        # The name this layout is known by: the `let` it is bound to, or — for the
        # `pub fn bind_group_layout(..) -> wgpu::BindGroupLayout` shape — the function's own name.
        owner = _innermost(spans, at, want_ret="wgpu::BindGroupLayout")
        name = None
        lead = _LET_BEFORE.search(src[max(0, m.start() - 200) : m.start()])
        if lead:
            name = lead.group(1)
        elif owner is not None:
            name = owner.name
        if name is None:
            name = f"<anonymous bgl @{line}>"

        # Absolute spans throughout — an element is located by index, never re-found by substring
        # search. `_split_top_level` strips its parts, so a multi-line entry literal does not appear
        # verbatim in the text it came from, and `find()` on it lands on the wrong brace or on -1.
        problems: list[str] = []
        body_at = at + 1
        span = next((sp for sp in _top_level_spans(body) if re.match(r"^entries\s*:", body[sp[0] : sp[1]])), None)
        if span is None:
            problems.append("descriptor has no `entries:` field")
            inner_at = inner_end = 0
        else:
            expr_at = body_at + span[0] + body[span[0] : span[1]].index(":") + 1
            expr = src[expr_at : body_at + span[1]]
            lead = len(expr) - len(expr.lstrip())
            stripped = expr.strip()
            if stripped.startswith("&"):
                lead += len(stripped) - len(stripped[1:].lstrip())
                stripped = stripped[1:].lstrip()
            if stripped.startswith("[") and stripped.endswith("]"):
                inner_at, inner_end = expr_at + lead + 1, expr_at + lead + len(stripped) - 1
            else:
                problems.append(f"`entries:` is {stripped[:60]!r}, not an array literal this audit can read")
                inner_at = inner_end = 0

        def build(env: dict[str, str], _at=inner_at, _end=inner_end, _p=problems) -> tuple[list[BindingEntry], list[str]]:
            entries: list[BindingEntry] = []
            probs = list(_p)
            for s, e in _top_level_spans(src[_at:_end]):
                el_at, el = _at + s, src[_at + s : _at + e]
                if el.startswith("wgpu::BindGroupLayoutEntry"):
                    entries.append(_parse_entry_literal(src, el_at + el.index("{"), env))
                    continue
                call = re.fullmatch(r"([A-Za-z_]\w*)\s*\((.*)\)", el, re.S)
                if call and call.group(1) in builders:
                    params, lit_at = builders[call.group(1)]
                    args = [_resolve_ident(a, env) for a in _split_top_level(call.group(2))]
                    if len(args) != len(params):
                        probs.append(f"`{el[:50]}` passes {len(args)} argument(s) to {len(params)} parameter(s)")
                        continue
                    entries.append(_parse_entry_literal(src, lit_at, dict(zip(params, args))))
                    continue
                if re.fullmatch(r"[A-Za-z_]\w*", el) and el in named:
                    entries.append(_parse_entry_literal(src, named[el], env))
                    continue
                probs.append(f"entry expression {el[:60]!r} is a shape this audit cannot read")
            return entries, probs

        label = _string(_field(body, "label") or "")
        # A layout helper that is itself parametric — `fn bgl(device, ty) -> wgpu::BindGroupLayout`,
        # called once for the camera uniform and once for the instance storage buffer. One descriptor,
        # two different layouts; resolved per call site exactly as a parametric pipeline is.
        used = [p for p in (owner.params if owner else []) if p and re.search(rf"\b{re.escape(p)}\b", src[inner_at:inner_end])]
        if owner is not None and used:
            for call in re.finditer(rf"let\s+(\w+)\s*(?::[^=]+)?=\s*(?:[A-Za-z_]\w*\s*::\s*)*{re.escape(owner.name)}\s*\(", src):
                try:
                    close = _match_brace(src, call.end() - 1, "(", ")")
                except ValueError:
                    continue
                args = _split_top_level(src[call.end() : close - 1])
                site = _line_of(src, call.start())
                if len(args) != len(owner.params):
                    bgls[call.group(1)] = BindGroupLayout(
                        call.group(1), label, [], site,
                        [f"`{owner.name}` takes {len(owner.params)} parameter(s) and this call passes {len(args)}"],
                    )
                    continue
                ents, probs = build(dict(zip(owner.params, args)))
                bgls[call.group(1)] = BindGroupLayout(call.group(1), label, ents, site, probs)
            continue
        ents, probs = build({})
        bgls[name] = BindGroupLayout(name, label, ents, line, probs)

    # `let x = some::path::fn(..)` — recorded by callee name so a cross-file layout can be found.
    aliases: dict[str, str] = {}
    for m in _LET_CALL.finditer(src):
        callee = re.sub(r"\s+", "", m.group(2)).split("::")[-1]
        if m.group(1) not in bgls:
            aliases[m.group(1)] = callee
    return bgls, aliases


# ── bind groups ───────────────────────────────────────────────────────────────────────────────────
# The level below `bindings`, and the last one wgpu checks. `bindings` compares the SHADER against the
# LAYOUT; a bind group is the same contract stated a THIRD time — the actual resources handed over at
# `create_bind_group` — and wgpu compares it against the layout separately, at a different moment.
#
# Three failures live only here, and all three are silent to every check above:
#   * a `TextureView` bound where the layout declares a `Buffer` (or any other class swap);
#   * an entry at an index the layout does not declare;
#   * a layout entry no bind group entry answers (wgpu requires the sets to correspond exactly).
# Proved before this was written: six such drifts applied to a copy of this crate, and the audit
# reported `0 blocking` for all six (`progress/gpu-bind-group-resources/02-before-mutation-matrix.txt`).
#
# **What a BindGroupEntry states, and no more.** `BindingResource::Buffer` / `.as_entire_binding()`
# says "a buffer" and nothing about uniform-vs-storage — wgpu resolves that against the buffer's
# USAGE FLAGS, set at `create_buffer`, which is neither in the layout nor in the group. Likewise a
# `TextureView`'s format and sample count are properties of the texture, not of this descriptor. So
# the comparison is at RESOURCE-CLASS granularity, and `audit.py` says so where it reports the count:
# a buffer bound where the layout wants a buffer, with the wrong usage flags, is still invisible here.

_BG_DESC = re.compile(r"create_bind_group\s*\(\s*&?\s*wgpu::BindGroupDescriptor\s*\{")
_BG_ENTRY_LIT = re.compile(r"wgpu::BindGroupEntry\s*\{")

# `BindingResource` variant → the layout family it can satisfy. `TextureView` answers both `Texture`
# and `StorageTexture`, which is why the map is to a class rather than to a kind.
_RESOURCE_CLASS = {
    "Buffer": "buffer",
    "BufferArray": "buffer",
    "Sampler": "sampler",
    "SamplerArray": "sampler",
    "TextureView": "texture",
    "TextureViewArray": "texture",
    "AccelerationStructure": "acceleration-structure",
}


def _resource_class(expr: str | None, env: dict[str, str]) -> tuple[str, list[str]]:
    if expr is None:
        return "", ["entry has no `resource:` field"]
    e = _resolve_ident(expr, env).strip()
    m = re.search(r"BindingResource\s*::\s*(\w+)", e)
    if m:
        cls = _RESOURCE_CLASS.get(m.group(1))
        if cls:
            return cls, []
        return "", [f"binding resource `{m.group(1)}` is not one this audit knows"]
    # `buf.as_entire_binding()` — the same `BindingResource::Buffer`, spelled as a method.
    if re.search(r"\.\s*as_entire_binding\s*\(", e):
        return "buffer", []
    return "", [f"unreadable binding resource {e[:60]!r}"]


def _parse_bind_group_entry(src: str, brace_at: int, env: dict[str, str]) -> BindGroupUse:
    body = src[brace_at + 1 : _match_brace(src, brace_at) - 1]
    problems: list[str] = []
    raw = _field(body, "binding")
    binding: int | None = None
    if raw is None:
        problems.append("entry has no `binding:` field")
    else:
        b = _resolve_ident(raw, env).strip()
        if _UINT.match(b):
            binding = int(b)
        else:
            problems.append(f"binding index {b[:40]!r} is not a literal")
    kind, kind_problems = _resource_class(_field(body, "resource"), env)
    return BindGroupUse(binding, kind, _line_of(src, brace_at), problems + kind_problems)


def _bg_entry_literal_sites(src: str) -> list[int]:
    """The `{` of every `wgpu::BindGroupEntry { .. }` VALUE — not a `-> wgpu::BindGroupEntry` body.

    The same guard `_entry_literal_sites` needs, for the same reason: a return type and a struct
    literal are the same text, and reading one as the other makes the gate fail about code it is not
    auditing.
    """
    return [m.end() - 1 for m in _BG_ENTRY_LIT.finditer(src) if not src[: m.start()].rstrip().endswith("->")]


def _parse_bind_groups(src: str) -> list[BindGroup]:
    spans = _callable_spans(src)
    named: dict[str, int] = {
        m.group(1): m.end() - 1
        for m in re.finditer(r"let\s+(\w+)\s*(?::[^=|]+)?=\s*wgpu::BindGroupEntry\s*\{", src)
    }
    builders: dict[str, tuple[list[str], int]] = {}
    for at in _bg_entry_literal_sites(src):
        owner = _innermost(spans, at)
        if owner is None or not owner.params:
            continue
        body = src[at : _match_brace(src, at)]
        if any(re.search(rf"\b{re.escape(p)}\b", body) for p in owner.params if p):
            builders.setdefault(owner.name, (owner.params, at))

    out: list[BindGroup] = []
    for m in _BG_DESC.finditer(src):
        at = m.end() - 1
        body_at = at + 1
        body = src[body_at : _match_brace(src, at) - 1]
        problems: list[str] = []
        label = _string(_field(body, "label") or "")
        layout_expr = (_field(body, "layout") or "").strip()
        if not layout_expr:
            problems.append("descriptor has no `layout:` field")

        span = next(
            (sp for sp in _top_level_spans(body) if re.match(r"^entries\s*:", body[sp[0] : sp[1]])), None
        )
        entries: list[BindGroupUse] = []
        if span is None:
            problems.append("descriptor has no `entries:` field")
        else:
            expr_at = body_at + span[0] + body[span[0] : span[1]].index(":") + 1
            expr = src[expr_at : body_at + span[1]]
            lead = len(expr) - len(expr.lstrip())
            stripped = expr.strip()
            if stripped.startswith("&"):
                lead += len(stripped) - len(stripped[1:].lstrip())
                stripped = stripped[1:].lstrip()
            if not (stripped.startswith("[") and stripped.endswith("]")):
                problems.append(
                    f"`entries:` is {stripped[:60]!r}, not an array literal this audit can read"
                )
            else:
                inner_at = expr_at + lead + 1
                inner_end = expr_at + lead + len(stripped) - 1
                for s, e in _top_level_spans(src[inner_at:inner_end]):
                    el_at, el = inner_at + s, src[inner_at + s : inner_at + e]
                    if el.startswith("wgpu::BindGroupEntry"):
                        entries.append(_parse_bind_group_entry(src, el_at + el.index("{"), {}))
                        continue
                    call = re.fullmatch(r"([A-Za-z_]\w*)\s*\((.*)\)", el, re.S)
                    if call and call.group(1) in builders:
                        params, lit_at = builders[call.group(1)]
                        args = _split_top_level(call.group(2))
                        if len(args) != len(params):
                            problems.append(
                                f"`{el[:50]}` passes {len(args)} argument(s) to {len(params)} parameter(s)"
                            )
                            continue
                        entries.append(_parse_bind_group_entry(src, lit_at, dict(zip(params, args))))
                        continue
                    if re.fullmatch(r"[A-Za-z_]\w*", el) and el in named:
                        entries.append(_parse_bind_group_entry(src, named[el], {}))
                        continue
                    problems.append(f"entry expression {el[:60]!r} is a shape this audit cannot read")
        out.append(BindGroup(label, layout_expr, entries, _line_of(src, m.start()), at, problems))
    return out


# ── string constants, for shader sources the host rewrites at start-up ────────────────────────────
# `render.rs` does not hand `ssao.wgsl` to wgpu as written. With MSAA off it substitutes the block
# between two marker comments, because WGSL types `texture_depth_multisampled_2d` and
# `texture_depth_2d` differently and one module cannot hold both. So one of the two shaders that
# actually reach the GPU exists only at run time — and an audit that reads the file at rest has
# never seen it. Proved: the substituted block's binding index moved to @binding(9) and its type
# swapped to a sampled float texture, twice a module wgpu would refuse, and the audit passed both
# (`progress/gpu-bind-group-resources/02-before-mutation-matrix.txt`).

_STR_CONST = re.compile(r"\bconst\s+([A-Za-z_]\w*)\s*:\s*&(?:'static\s+)?str\s*=\s*")

_ESCAPES = {"n": "\n", "t": "\t", "r": "\r", "0": "\0", "\\": "\\", '"': '"', "'": "'"}


def _rust_string_at(src: str, i: int) -> tuple[str, int] | None:
    """Decode the Rust string literal starting at `i` → (value, index after the closing quote)."""
    if src.startswith("r", i):  # r"..." / r#"..."#
        j = i + 1
        hashes = 0
        while j < len(src) and src[j] == "#":
            hashes += 1
            j += 1
        if j >= len(src) or src[j] != '"':
            return None
        close = src.find('"' + "#" * hashes, j + 1)
        return (src[j + 1 : close], close + hashes + 1) if close > 0 else None
    if not src.startswith('"', i):
        return None
    out: list[str] = []
    j = i + 1
    while j < len(src):
        c = src[j]
        if c == '"':
            return "".join(out), j + 1
        if c == "\\":
            nxt = src[j + 1 : j + 2]
            if nxt == "\n":
                # A line continuation: the newline AND the next line's leading whitespace vanish.
                j += 2
                while j < len(src) and src[j] in " \t":
                    j += 1
                continue
            out.append(_ESCAPES.get(nxt, nxt))
            j += 2
            continue
        out.append(c)
        j += 1
    return None


def string_consts(src: str) -> dict[str, str]:
    """Every `const NAME: &str = "…";` with its decoded value."""
    out: dict[str, str] = {}
    for m in _STR_CONST.finditer(src):
        got = _rust_string_at(src, m.end())
        if got is not None:
            out[m.group(1)] = got[0]
    return out


# ── resolving a `layout:` expression to the layout(s) it can be ───────────────────────────────────
# `layout` is a PARAMETER at 10 of the 15 `create_bind_group` sites in this renderer, so a resolver
# that stopped at "is this a name I know?" would have reported UNRESOLVED for two thirds of the tree
# and been waived within a week. It substitutes across call sites instead, transitively.
#
# Three rules keep the substitution honest, and each was written because its absence produced a wrong
# answer on the real tree:
#   * A call site that CANNOT be resolved poisons the whole result. `make_inst_bg` is called from
#     `InstanceBuf::new` AND `LightBuf::new`; taking "the sites that did resolve agree" answered
#     `inst_bgl` alone and would have audited one layout while believing it had covered both.
#   * The answer is a SET, not a name — that same helper genuinely builds a bind group for
#     `inst_bgl`, `lights_bgl` and `mesh_inst_bgl`, and it has to satisfy all of them.
#   * A same-named function is not the function. `Targets::create`, `crate::ibl::create` and
#     `std::fs::File::create` all match `create\s*\(`; `InstanceBuf::upload` and `LightBuf::upload`
#     both match `.upload(`. Associated functions are matched by `impl` type, methods by the
#     receiver's type, free functions by module. A rejected call site is not a gap — it is a
#     different function — so it is counted, not reported.

_IMPL_BLOCK = re.compile(r"\bimpl\b[^{;]*?\b([A-Z]\w*)\s*\{")


def _let_binding(src: str, name: str, scope_start: int, at: int) -> tuple[str, str] | None:
    """The last `let name[: Ty] = <expr>;` before `at` within the scope → (expr, type annotation)."""
    pat = re.compile(rf"let\s+(?:mut\s+)?{re.escape(name)}\s*(:\s*[^=;]+?)?=\s*")
    best: tuple[int, str] | None = None
    for m in pat.finditer(src[scope_start:at]):
        best = (scope_start + m.end(), (m.group(1) or "")[1:].strip())
    if best is None:
        return None
    end = src.find(";", best[0])
    return (src[best[0] : end].strip() if end > 0 else ""), best[1]


class LayoutResolver:
    """Resolve a bind group's `layout:` expression to the bind group layout(s) it can be.

    Cross-file by construction: `ibl.rs`'s bind group takes its layout from a parameter whose only
    caller is in `render.rs`, two hops away through `ibl::create`.
    """

    def __init__(self, files: dict[str, str], layouts: set[str]) -> None:
        self.files = files
        self.layouts = layouts
        self._spans = {rel: _callable_spans(src) for rel, src in files.items()}

    # -- scope helpers ---------------------------------------------------------------------------
    def _enclosing(self, rel: str, at: int) -> list[_Callable]:
        out = [c for c in self._spans[rel] if c.start <= at < c.end]
        out.sort(key=lambda c: c.end - c.start)
        return out

    def _impl_of(self, rel: str, at: int) -> str | None:
        src = self.files[rel]
        best: tuple[int, str] | None = None
        for m in _IMPL_BLOCK.finditer(src):
            if m.start() > at:
                break
            try:
                end = _match_brace(src, m.end() - 1)
            except ValueError:
                continue
            if m.start() <= at < end and (best is None or m.start() > best[0]):
                best = (m.start(), m.group(1))
        return best[1] if best else None

    def _receiver_is(self, rel: str, at: int, recv: str, ty: str) -> bool:
        base = re.split(r"[.\[]", recv)[0]
        if base == "self":
            return self._impl_of(rel, at) == ty
        for c in self._enclosing(rel, at):
            b = _let_binding(self.files[rel], base, c.start, at)
            if b is None:
                continue
            rhs, ann = b
            if ann and re.search(rf"\b{re.escape(ty)}\b", ann):
                return True  # `let mut mesh_inst: Vec<InstanceBuf> = Vec::new();`
            rhs = rhs.lstrip("&").strip()
            return bool(re.match(rf"(?:\w+\s*::\s*)*{re.escape(ty)}\s*(::|\{{)", rhs))
        return False

    def _call_sites(self, rel: str, c: _Callable) -> tuple[list[tuple[str, int, list[str]]], int]:
        """Every call of `c`, and the number of same-named calls rejected as another function."""
        params = list(c.params)
        is_method = bool(params) and params[0].endswith("self")
        owner_ty = self._impl_of(rel, c.start)
        module = rel.rsplit("/", 1)[-1][:-3]
        pat = re.compile(
            (r"([\w\.\[\]]+)\s*\.\s*" if is_method else r"(?<![\w])((?:\w+\s*::\s*)*)")
            + re.escape(c.name)
            + r"\s*\("
        )
        out: list[tuple[str, int, list[str]]] = []
        rejected = 0
        for r, s in self.files.items():
            for m in pat.finditer(s):
                if s[: m.start()].rstrip().endswith("fn"):
                    continue  # the declaration
                if is_method:
                    if owner_ty and not self._receiver_is(r, m.start(), m.group(1), owner_ty):
                        rejected += 1
                        continue
                else:
                    path = [p.strip() for p in m.group(1).split("::") if p.strip()]
                    if owner_ty:  # an associated fn: only `T::f`, or `Self::f` inside `impl T`
                        # `Self` IS SCOPED, and reading it as a spelling of one type is how a second
                        # `fn new` in an unrelated `impl` gets read as a call of this one. The method
                        # branch above has always resolved a `self.` receiver through `_impl_of`;
                        # this is the same rule for the associated form, which had it only for the
                        # explicit `T::f` half. (Found 2026-08-28: `Frame::whole`'s
                        # `Self::new(aspect, FULL_FRAME)` was attributed to `InstanceBuf::new` AND
                        # `LightBuf::new`, so `make_inst_bg`'s `layout` parameter resolved to a
                        # `const [f32; 4]` and both instance bind groups reported UNREAD.)
                        named = path[-1] if path else ""
                        if named == "Self":
                            if self._impl_of(r, m.start()) != owner_ty:
                                rejected += 1
                                continue
                        elif named != owner_ty:
                            rejected += 1
                            continue
                    elif (path and path[-1] not in (module, "crate", "super", "self")) or (
                        not path and r != rel
                    ):
                        rejected += 1  # a free fn is in scope unqualified only in its own module
                        continue
                try:
                    close = _match_brace(s, m.end() - 1, "(", ")")
                except ValueError:
                    continue
                out.append((r, m.start(), _split_top_level(s[m.end() : close - 1])))
        return out, rejected

    def _field_initialisers(self, fld: str) -> list[tuple[str, int, str]]:
        out: list[tuple[str, int, str]] = []
        for r, s in self.files.items():
            for m in re.finditer(rf"(?<![\w:]){re.escape(fld)}\s*:\s*", s):
                k = m.end()
                depth = 0
                while k < len(s):
                    ch = s[k]
                    if ch in "([{":
                        depth += 1
                    elif ch in ")]}":
                        if depth == 0:
                            break
                        depth -= 1
                    elif ch in ",;" and depth == 0:
                        break
                    k += 1
                out.append((r, m.start(), s[m.end() : k].strip()))
        return out

    # -- the resolver ----------------------------------------------------------------------------
    def resolve(
        self, rel: str, at: int, expr: str, depth: int = 0, seen: tuple = ()
    ) -> tuple[set[str], list[str]]:
        e = re.sub(r"^'\w+\s+", "", expr.strip().lstrip("&").strip())
        if depth > 6:
            return set(), [f"gave up following `{e[:30]}` after 6 substitutions"]
        if not e:
            return set(), ["the `layout:` expression is empty"]
        if e in self.layouts:
            return {e}, []
        call = re.fullmatch(r"(?:\w+\s*::\s*)*(\w+)\s*\(.*\)", e, re.S)
        if call and call.group(1) in self.layouts:
            return {call.group(1)}, []  # `crate::ibl::bind_group_layout(&device)`
        if re.fullmatch(r"\w+(\.\w+)+", e):  # `thumb.post_bgl2` — a struct field
            got: set[str] = set()
            for r2, at2, v in self._field_initialisers(e.rsplit(".", 1)[1]):
                got |= self.resolve(r2, at2, v, depth + 1, seen)[0]
            if got:
                return got, []
            return set(), [f"field `{e}`: no initialiser of it resolves to a bind group layout"]
        if not re.fullmatch(r"[A-Za-z_]\w*", e):
            return set(), [f"`{e[:40]}` is not a name this audit can follow"]

        chain = self._enclosing(rel, at)
        for c in chain:
            b = _let_binding(self.files[rel], e, c.start, at)
            if b is not None:
                return self.resolve(rel, at, b[0], depth + 1, seen)
        for c in chain:
            params = list(c.params)
            if e not in params:
                continue
            is_method = bool(params) and params[0].endswith("self")
            idx = params.index(e) - (1 if is_method else 0)
            if idx < 0:
                return set(), ["the layout is `self`, which this audit does not follow"]
            key = (rel, c.name, idx)
            if key in seen:
                return set(), [f"`{c.name}` passes its own layout parameter to itself"]
            sites, rejected = self._call_sites(rel, c)
            sites = [s for s in sites if not (s[0] == rel and c.start <= s[1] < c.end)]
            if not sites:
                extra = f" ({rejected} same-named call(s) belong to another function)" if rejected else ""
                return set(), [f"`{c.name}` has no call site this audit can see{extra}"]
            got, why = set(), []
            for r2, at2, args in sites:
                line = _line_of(self.files[r2], at2)
                if idx >= len(args):
                    why.append(f"{r2}:{line} passes {len(args)} argument(s) to `{c.name}`")
                    continue
                g, w = self.resolve(r2, at2, args[idx], depth + 1, seen + (key,))
                # BOTH, always. `if g: got |= g else: why += w` reads naturally and is wrong: a frame
                # that resolved one of its two call sites returns a name AND a complaint, and taking
                # only the name discards the complaint one level up — so a partially-readable chain
                # ends at the top looking fully resolved. Caught by the mutation matrix, in the
                # mutant written to test exactly this rule.
                got |= g
                why += [f"{r2}:{line} {x}" for x in w]
            return got, why
        return set(), [f"`{e}` is not a bind group layout, a local, or a parameter here"]


def _template_is_parametric(tpl: _Template, params: list[str]) -> bool:
    joined = " ".join(
        x or ""
        for x in (tpl.vertex_expr, tpl.fragment_expr, tpl.buffers_expr, tpl.layout_expr, tpl.topology_expr)
    )
    return any(re.search(rf"\b{re.escape(p)}\b", joined) for p in params if p)


_MATCH = re.compile(r"match\s+(\w+)\s*\{(.*)\}", re.S)


def _resolve(expr: str | None, env: dict[str, str]) -> str | None:
    if expr is None:
        return None
    e = expr.strip()
    inner = e
    if inner.startswith("Some(") and inner.endswith(")"):
        inner = inner[5:-1].strip()
    m = _MATCH.fullmatch(inner)
    if m:
        subject = env.get(m.group(1), m.group(1))
        subject_lit = _string(subject) or subject.strip().strip('"')
        default = None
        for arm in _split_top_level(m.group(2)):
            if "=>" not in arm:
                continue
            pat, val = arm.split("=>", 1)
            pat, val = pat.strip(), val.strip()
            if pat == "_":
                default = _string(val) or val
            elif (_string(pat) or pat.strip('"')) == subject_lit:
                return _string(val) or val
        return default
    for k, v in env.items():
        if re.fullmatch(rf"&?\s*{re.escape(k)}", inner):
            return _string(v) or v.strip()
    return _string(inner) or inner


def _concrete(tpl: _Template, env: dict[str, str]) -> Pipeline:
    label = _resolve(tpl.label, env)
    vtx = _resolve(tpl.vertex_expr, env)
    frg = _resolve(tpl.fragment_expr, env)
    bufs = _resolve(tpl.buffers_expr, env)
    lay = _resolve(tpl.layout_expr, env)
    topo = _resolve(tpl.topology_expr, env)
    if topo:
        topo = topo.replace("wgpu::PrimitiveTopology::", "").strip()
    unresolved = []
    for what, val in (("vertex entry_point", vtx), ("fragment entry_point", frg)):
        if val is not None and not re.fullmatch(r"[A-Za-z_]\w*", val or ""):
            unresolved.append(f"{what} = {val}")
    return Pipeline(
        label=(label or "<unlabelled>").strip('"'),
        vertex_entry=vtx if vtx and re.fullmatch(r"[A-Za-z_]\w*", vtx) else None,
        fragment_entry=frg if frg and re.fullmatch(r"[A-Za-z_]\w*", frg) else None,
        buffers=bufs,
        layout=_ident(lay) if lay else None,
        line=tpl.line,
        topology=topo if topo and re.fullmatch(r"[A-Za-z_]\w*", topo) else None,
        unresolved=unresolved,
    )
