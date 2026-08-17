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


def _split_top_level(text: str, sep: str = ",") -> list[str]:
    """Split on `sep` at bracket depth zero.

    `>` is a bracket in `Vec<A, B>` and an arrow in `=>` / `->`. Counting the arrow forms as
    closers is not a cosmetic bug: it drives the depth negative, so the very next comma reads as
    top level and the field is silently truncated mid-expression — which is how a `match` arm
    selecting a fragment entry point went unread.
    """
    out, depth, cur, i = [], 0, [], 0
    while i < len(text):
        ch = text[i]
        if ch == '"':
            cur.append(ch)
            i += 1
            while i < len(text) and text[i] != '"':
                cur.append(text[i])
                i += 2 if text[i] == "\\" else 1
            if i < len(text):
                cur.append(text[i])
                i += 1
            continue
        arrow = ch == ">" and i > 0 and text[i - 1] in "=-"
        if ch in "<([{":
            depth += 1
        elif ch in ")]}" or (ch == ">" and not arrow):
            depth -= 1
        if ch == sep and depth == 0:
            out.append("".join(cur))
            cur = []
        else:
            cur.append(ch)
        i += 1
    out.append("".join(cur))
    return [s for s in (p.strip() for p in out) if s]


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
    return RustFile(path, src, vertex_layouts, pipeline_layouts, pipelines, structs, shaders)


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
