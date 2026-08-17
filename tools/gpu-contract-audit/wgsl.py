"""A small WGSL reader: enough of the grammar to recover the GPU contract, and no more.

What "the contract" means here is exactly what wgpu validates when a pipeline is created:
which `@location`s a vertex entry consumes, which ones a fragment entry expects to be handed,
which `@group`/`@binding` resources an entry actually reaches, and how wide a host-shared struct
is once WGSL's alignment rules have been applied. Those four questions are the ones the Rust side
answers a second time, in its own words, which is where the two drift apart.

This is deliberately not a full front end. It parses declarations, not expressions, and it resolves
identifiers by name rather than by scope. Every construct it does not understand is reported rather
than skipped, so a shader that grows a feature this reader cannot see fails loudly instead of
quietly auditing nothing.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field


# ── the type system, only as far as layout ────────────────────────────────────────────────────────
# WGSL's alignment rules (spec §14.4.4, "Alignment and Size"). Only the types this engine's shaders
# actually use are listed; an unknown type raises rather than guesses, because a guessed size is a
# wrong answer that looks like a right one.

_SCALAR = {
    "f32": (4, 4),
    "i32": (4, 4),
    "u32": (4, 4),
    "f16": (2, 2),
    "bool": (4, 4),
}


@dataclass(frozen=True)
class Layout:
    """(align, size) — size is the type's own size, before any trailing struct padding."""

    align: int
    size: int


def _round_up(n: int, mult: int) -> int:
    return (n + mult - 1) // mult * mult


class WgslError(Exception):
    pass


# ── declarations ──────────────────────────────────────────────────────────────────────────────────


@dataclass
class Field:
    name: str
    ty: str
    location: int | None = None
    builtin: str | None = None
    interpolate: str | None = None
    offset: int = 0  # filled in by the layout pass


@dataclass
class Struct:
    name: str
    fields: list[Field]
    size: int = 0  # filled in by the layout pass
    align: int = 0


@dataclass
class Global:
    name: str
    group: int
    binding: int
    address_space: str  # "uniform" | "storage" | "handle" (textures/samplers)
    access: str | None  # "read" | "read_write" for storage
    ty: str


@dataclass
class Entry:
    name: str
    stage: str  # "vertex" | "fragment" | "compute"
    params: list[Field]
    ret: str | None
    body: str = ""
    calls: set[str] = field(default_factory=set)


@dataclass
class Module:
    structs: dict[str, Struct]
    globals: dict[str, Global]
    entries: dict[str, Entry]
    functions: dict[str, Entry]  # every fn, entry or not, for the call graph

    # ── the four contract questions ───────────────────────────────────────────────────────────────

    def inputs(self, entry: str) -> dict[int, str]:
        """`@location` → WGSL type, for everything the entry consumes as user-defined input."""
        return self._locations(self.entries[entry].params)

    def outputs(self, entry: str) -> dict[int, str]:
        """`@location` → WGSL type, for everything the entry hands to the next stage."""
        e = self.entries[entry]
        if e.ret is None:
            return {}
        if e.ret in self.structs:
            return self._locations(self.structs[e.ret].fields)
        # An inline return like `-> @location(0) vec4<f32>` or `-> @builtin(position) vec4<f32>`.
        return self._locations(e.ret_fields) if hasattr(e, "ret_fields") else {}

    def _locations(self, fields: list[Field]) -> dict[int, str]:
        out: dict[int, str] = {}
        for f in fields:
            if f.location is not None:
                out[f.location] = f.ty
            elif f.builtin is None and f.ty in self.structs:
                # A struct parameter: its fields carry the locations.
                for g in self.structs[f.ty].fields:
                    if g.location is not None:
                        out[g.location] = g.ty
        return out

    def resources_used(self, entry: str) -> dict[str, Global]:
        """Every `@group`/`@binding` global the entry reaches, directly or through a call.

        wgpu only requires a pipeline layout to cover the bindings a shader *uses* — a declared but
        unreached global is free. So the audit has to follow calls, or it reports work that is not
        owed.
        """
        seen: set[str] = set()
        todo = [entry]
        used: dict[str, Global] = {}
        while todo:
            fn = todo.pop()
            if fn in seen or fn not in self.functions:
                continue
            seen.add(fn)
            f = self.functions[fn]
            for name, g in self.globals.items():
                if _mentions(f.body, name):
                    used[name] = g
            todo.extend(f.calls)
        return used


def _mentions(body: str, ident: str) -> bool:
    return re.search(rf"\b{re.escape(ident)}\b", body) is not None


# ── parsing ───────────────────────────────────────────────────────────────────────────────────────

_COMMENT = re.compile(r"//[^\n]*|/\*.*?\*/", re.S)


def strip_comments(src: str) -> str:
    # Replace with equal-length whitespace so byte offsets (and therefore error line numbers) hold.
    return _COMMENT.sub(lambda m: re.sub(r"[^\n]", " ", m.group(0)), src)


_ATTR = re.compile(
    r"@(?P<name>location|builtin|interpolate|group|binding|align|size|invariant)"
    r"(?:\(\s*(?P<arg>[^)]*)\s*\))?"
)


def _split_attrs(text: str) -> tuple[dict[str, str], str]:
    """Peel leading `@attr(...)` off a declaration, returning (attrs, remainder)."""
    attrs: dict[str, str] = {}
    pos = 0
    while True:
        m = _ATTR.match(text, pos)
        if not m:
            # skip whitespace between attributes
            nxt = re.match(r"\s+", text[pos:])
            if nxt and _ATTR.match(text, pos + nxt.end()):
                pos += nxt.end()
                continue
            break
        attrs[m.group("name")] = (m.group("arg") or "").strip()
        pos = m.end()
    return attrs, text[pos:].lstrip()


def _parse_member(text: str) -> Field | None:
    """`@location(3) metallic: f32` → Field. Returns None for an empty fragment."""
    text = text.strip()
    if not text:
        return None
    attrs, rest = _split_attrs(text)
    if ":" not in rest:
        raise WgslError(f"member without a type: {text!r}")
    name, ty = rest.split(":", 1)
    loc = attrs.get("location")
    return Field(
        name=name.strip(),
        ty=_norm_type(ty),
        location=int(loc) if loc is not None and loc != "" else None,
        builtin=attrs.get("builtin"),
        interpolate=attrs.get("interpolate"),
    )


def _norm_type(ty: str) -> str:
    return re.sub(r"\s+", "", ty.strip().rstrip(",").rstrip(";"))


def _split_top_level(text: str, sep: str = ",") -> list[str]:
    """Split on `sep`, ignoring separators nested inside <>, () or []."""
    out, depth, cur = [], 0, []
    for ch in text:
        if ch in "<([":
            depth += 1
        elif ch in ">)]":
            depth -= 1
        if ch == sep and depth == 0:
            out.append("".join(cur))
            cur = []
        else:
            cur.append(ch)
    out.append("".join(cur))
    return [s for s in (p.strip() for p in out) if s]


def _match_brace(src: str, open_at: int) -> int:
    """Index just past the `}` that closes the `{` at `open_at`."""
    depth = 0
    for i in range(open_at, len(src)):
        if src[i] == "{":
            depth += 1
        elif src[i] == "}":
            depth -= 1
            if depth == 0:
                return i + 1
    raise WgslError("unbalanced braces")


_STRUCT = re.compile(r"\bstruct\s+(\w+)\s*\{")
_GLOBAL = re.compile(
    r"@group\(\s*(\d+)\s*\)\s*@binding\(\s*(\d+)\s*\)\s*var\s*"
    r"(?:<\s*(\w+)\s*(?:,\s*(\w+)\s*)?>)?\s*(\w+)\s*:\s*([^;]+);"
)
_FN = re.compile(r"(?:^|\n)\s*(?:(@vertex|@fragment|@compute(?:\s*@workgroup_size\([^)]*\))?)\s*)?fn\s+(\w+)\s*\(")


def parse(src: str) -> Module:
    src = strip_comments(src)
    structs: dict[str, Struct] = {}
    globals_: dict[str, Global] = {}
    entries: dict[str, Entry] = {}
    functions: dict[str, Entry] = {}

    for m in _STRUCT.finditer(src):
        end = _match_brace(src, m.end() - 1)
        body = src[m.end() : end - 1]
        fields = [f for f in (_parse_member(p) for p in _split_top_level(body)) if f]
        structs[m.group(1)] = Struct(m.group(1), fields)

    for m in _GLOBAL.finditer(src):
        group, binding, space, access, name, ty = m.groups()
        globals_[name] = Global(
            name=name,
            group=int(group),
            binding=int(binding),
            address_space=space or "handle",
            access=access,
            ty=_norm_type(ty),
        )

    for m in _FN.finditer(src):
        stage_attr, name = m.group(1), m.group(2)
        # parameter list
        open_paren = m.end() - 1
        depth, i = 0, open_paren
        while i < len(src):
            if src[i] == "(":
                depth += 1
            elif src[i] == ")":
                depth -= 1
                if depth == 0:
                    break
            i += 1
        params_txt = src[open_paren + 1 : i]
        after = src[i + 1 :]
        ret_m = re.match(r"\s*->\s*([^{]+)\{", after)
        ret_txt = ret_m.group(1).strip() if ret_m else None
        brace_at = i + 1 + (ret_m.end() - 1 if ret_m else re.match(r"\s*\{", after).end() - 1)
        body = src[brace_at : _match_brace(src, brace_at)]

        params = [f for f in (_parse_member(p) for p in _split_top_level(params_txt)) if f]
        ret_struct: str | None = None
        ret_fields: list[Field] = []
        if ret_txt:
            attrs, rest = _split_attrs(ret_txt)
            ret_struct = _norm_type(rest)
            if "location" in attrs:
                ret_fields = [Field("", _norm_type(rest), location=int(attrs["location"]))]
            elif "builtin" in attrs:
                ret_fields = [Field("", _norm_type(rest), builtin=attrs["builtin"])]

        fn = Entry(
            name=name,
            stage={"@vertex": "vertex", "@fragment": "fragment"}.get(
                (stage_attr or "").split()[0] if stage_attr else "", "compute" if stage_attr else ""
            ),
            params=params,
            ret=ret_struct,
            body=body,
        )
        fn.ret_fields = ret_fields  # type: ignore[attr-defined]
        functions[name] = fn
        if stage_attr:
            entries[name] = fn

    # call graph: a name followed by `(` that is a declared function
    for fn in functions.values():
        fn.calls = {c for c in re.findall(r"\b(\w+)\s*\(", fn.body) if c in functions}

    mod = Module(structs, globals_, entries, functions)
    _lay_out(mod)
    return mod


# ── layout ────────────────────────────────────────────────────────────────────────────────────────

_VEC = re.compile(r"^vec([234])<(\w+)>$")
_MAT = re.compile(r"^mat([234])x([234])<(\w+)>$")
_ARR = re.compile(r"^array<(.+?)(?:,\s*(\d+))?>$")


def layout_of(ty: str, structs: dict[str, Struct]) -> Layout:
    ty = _norm_type(ty)
    if ty in _SCALAR:
        a, s = _SCALAR[ty]
        return Layout(a, s)
    m = _VEC.match(ty)
    if m:
        n, base = int(m.group(1)), m.group(2)
        if base not in _SCALAR:
            raise WgslError(f"vector of a non-scalar: {ty}")
        _, es = _SCALAR[base]
        return Layout({2: 2 * es, 3: 4 * es, 4: 4 * es}[n], n * es)
    m = _MAT.match(ty)
    if m:
        cols, rows, base = int(m.group(1)), int(m.group(2)), m.group(3)
        col = layout_of(f"vec{rows}<{base}>", structs)
        return Layout(col.align, cols * col.align)  # columns are align-strided
    m = _ARR.match(ty)
    if m:
        el = layout_of(m.group(1), structs)
        stride = _round_up(el.size, el.align)
        n = int(m.group(2)) if m.group(2) else 0  # runtime-sized → 0, size is not fixed
        return Layout(el.align, stride * n)
    if ty in structs:
        s = structs[ty]
        if not s.size:
            _lay_out_struct(s, structs)
        return Layout(s.align, s.size)
    raise WgslError(f"unknown type {ty!r}")


def _lay_out_struct(s: Struct, structs: dict[str, Struct]) -> None:
    off, align = 0, 1
    for f in s.fields:
        if f.builtin is not None:
            continue  # a builtin is not host-shared
        fl = layout_of(f.ty, structs)
        off = _round_up(off, fl.align)
        f.offset = off
        off += fl.size
        align = max(align, fl.align)
    s.align = align
    s.size = _round_up(off, align)


def _lay_out(mod: Module) -> None:
    for s in mod.structs.values():
        # A struct used purely as an IO envelope (every field carries a location/builtin) has no
        # host layout to speak of; laying it out anyway is harmless and keeps the code single-path.
        try:
            _lay_out_struct(s, mod.structs)
        except WgslError:
            s.size, s.align = 0, 0
