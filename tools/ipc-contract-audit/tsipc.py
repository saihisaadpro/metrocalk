"""The TypeScript half of the IPC contract: what the UI believes a command is.

`invoke<T>("name", { key })` states three things the Rust never sees — the command's *name* as a
string, the *keys* of its argument object, and *T*, the shape the caller will read fields out of.
TypeScript checks `T` against the code that consumes it and against nothing else: `T` is an
assertion about a foreign process, and `tsc` accepts whatever the author writes.

This module recovers those three statements, plus the interfaces `T` refers to, so they can be
compared against the Rust.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field


@dataclass
class Invocation:
    cmd: str | None  # None => the name is computed, so no static check is possible
    targ: str | None  # the `T` in invoke<T>, verbatim
    keys: list[str]
    line: int
    file: str
    raw: str = ""
    spread: bool = False  # `...opts` in the argument object: the key set is not fully known


@dataclass
class TsType:
    name: str
    fields: list[tuple[str, bool]]  # (key, optional)
    line: int
    file: str
    #: key -> the declared type expression. Needed to follow a reply a second level down; kept beside
    #: `fields` so every existing consumer of `(key, optional)` is untouched.
    field_types: dict[str, str] = field(default_factory=dict)


@dataclass
class TsIpc:
    invocations: list[Invocation] = field(default_factory=list)
    types: dict[str, TsType] = field(default_factory=dict)
    #: `export type X = <something not an object>` — tuples, unions, aliases. Kept separately
    #: because resolving one yields a *type expression* to re-enter, not a field list.
    aliases: dict[str, str] = field(default_factory=dict)


def strip_comments(src: str) -> str:
    """Remove block and line comments, preserving line count so reported lines stay true."""
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
            j = n if j < 0 else j
            out.append("")
            i = j
            continue
        out.append(c)
        i += 1
    return "".join(out)


def _match(src: str, i: int, opens: str, closes: str) -> int:
    depth, n = 0, len(src)
    while i < n:
        c = src[i]
        if c in "\"'`":
            j = i + 1
            while j < n and src[j] != c:
                j += 2 if src[j] == "\\" else 1
            i = j + 1
            continue
        if c in opens:
            depth += 1
        elif c in closes:
            depth -= 1
            if depth == 0:
                return i
        i += 1
    return -1


def split_top(src: str, seps: str = ",") -> list[str]:
    parts: list[str] = []
    depth = 0
    cur: list[str] = []
    i, n = 0, len(src)
    while i < n:
        c = src[i]
        if c in "\"'`":
            j = i + 1
            while j < n and src[j] != c:
                j += 2 if src[j] == "\\" else 1
            cur.append(src[i : j + 1])
            i = j + 1
            continue
        if c in "<([{":
            depth += 1
        elif c in ">)]}":
            depth -= 1
        if c in seps and depth == 0:
            parts.append("".join(cur))
            cur = []
        else:
            cur.append(c)
        i += 1
    if "".join(cur).strip():
        parts.append("".join(cur))
    return [p.strip() for p in parts if p.strip()]


# A method call (`core.invoke(..)`) OR a bare one (`invoke(..)`, from `@tauri-apps/api/core`). This
# repo's editor uses the first and its spikes use the second, so a partial migration between the two
# would have shrunk the audited surface silently: the coverage guard only fires when *no* call site
# is found at all, and half of them disappearing looks exactly like a smaller number in a header.
# `\b` keeps it off `reinvoke(` and the like.
_INVOKE = re.compile(r"(?:\.\s*|\b)invoke\s*")


def parse_invocations(src: str, rel: str) -> list[Invocation]:
    out: list[Invocation] = []
    for m in _INVOKE.finditer(src):
        # A declaration, not a call: `invoke<T = unknown>(cmd: string, args?: ..): Promise<T>`.
        if re.match(r"\s*<[^>]*=", src[m.end() :]):
            continue
        i = m.end()
        targ = None
        if i < len(src) and src[i] == "<":
            close = _match(src, i, "<", ">")
            if close < 0:
                continue
            targ = src[i + 1 : close].strip()
            i = close + 1
        while i < len(src) and src[i].isspace():
            i += 1
        if i >= len(src) or src[i] != "(":
            continue
        cp = _match(src, i, "(", ")")
        if cp < 0:
            continue
        line = src.count("\n", 0, m.start()) + 1
        parts = split_top(src[i + 1 : cp])
        if not parts:
            continue
        nm = re.fullmatch(r"""["'`]([A-Za-z_]\w*)["'`]""", parts[0].strip())
        if not nm:
            out.append(Invocation(None, targ, [], line, rel, raw=parts[0].strip()[:70]))
            continue
        keys: list[str] = []
        spread = False
        if len(parts) > 1:
            obj = parts[1].strip()
            if obj.startswith("{"):
                ce = _match(obj, 0, "{", "}")
                for kv in split_top(obj[1 : ce if ce > 0 else len(obj)]):
                    kv = kv.strip()
                    if kv.startswith("..."):
                        spread = True
                        continue
                    k = split_top(kv, ":")[0].strip().strip("\"'`")
                    if re.fullmatch(r"[A-Za-z_]\w*", k):
                        keys.append(k)
                    else:
                        spread = True  # computed key — the set is not fully known
            else:
                spread = True  # the argument object is a variable; its keys are not visible here
        out.append(Invocation(nm.group(1), targ, keys, line, rel, spread=spread))
    return out


_TYPE_ALIAS = re.compile(r"\bexport\s+type\s+([A-Za-z_]\w*)(?:<[^>=]*>)?\s*=\s*")


def parse_aliases(src: str, rel: str) -> dict[str, str]:
    """`export type X = <expr>;` where `<expr>` is not an object literal.

    A tuple (`[number, number]`) and a string-literal union (`"cinematic" | "cad"`) are both perfectly
    ordinary reply types. Recording only the `= {` form left both looking like types the audit had
    never heard of, which turned four checkable call sites into four "unchecked" lines.
    """
    out: dict[str, str] = {}
    for m in _TYPE_ALIAS.finditer(src):
        rhs = src[m.end() :]
        if rhs.lstrip().startswith("{"):
            continue
        end = 0
        depth = 0
        for i, c in enumerate(rhs[:4000]):
            if c in "<([{":
                depth += 1
            elif c in ">)]}":
                depth -= 1
            elif c == ";" and depth == 0:
                end = i
                break
            elif c == "\n" and depth == 0 and rhs[:i].strip():
                nxt = rhs[i:].lstrip()
                if not nxt.startswith("|") and not nxt.startswith("&"):
                    end = i
                    break
        if end:
            out[m.group(1)] = " ".join(rhs[:end].split())
    return out


def parse_types(src: str, rel: str) -> dict[str, TsType]:
    """`export interface X { .. }` and `export type X = { .. }` — the object shapes only."""
    types: dict[str, TsType] = {}
    for m in re.finditer(r"\bexport\s+interface\s+([A-Za-z_]\w*)(?:<[^>{]*>)?\s*(?:extends[^{]+)?\{", src):
        ob = src.index("{", m.end() - 1)
        cb = _match(src, ob, "{", "}")
        if cb < 0:
            continue
        fs, fts = _object_fields_typed(src[ob + 1 : cb])
        types[m.group(1)] = TsType(m.group(1), fs, src.count("\n", 0, m.start()) + 1, rel, field_types=fts)
    for m in re.finditer(r"\bexport\s+type\s+([A-Za-z_]\w*)(?:<[^>=]*>)?\s*=\s*\{", src):
        ob = src.index("{", m.end() - 1)
        cb = _match(src, ob, "{", "}")
        if cb < 0:
            continue
        fs, fts = _object_fields_typed(src[ob + 1 : cb])
        types.setdefault(
            m.group(1),
            TsType(m.group(1), fs, src.count("\n", 0, m.start()) + 1, rel, field_types=fts),
        )
    return types


def _object_fields(body: str) -> list[tuple[str, bool]]:
    return _object_fields_typed(body)[0]


def _object_fields_typed(body: str) -> tuple[list[tuple[str, bool]], dict[str, str]]:
    """The keys, and the type expression each one declares.

    A member's declared type is the only thing that lets the audit follow a reply past its first
    level. Parsed here, once, rather than re-derived by whoever needs it."""
    fields: list[tuple[str, bool]] = []
    types: dict[str, str] = {}
    for p in split_top(body, ";\n"):
        p = p.strip()
        if not p or p.startswith("["):
            continue  # an index signature accepts any key; it constrains nothing we can check
        fm = re.match(r"""^(?:readonly\s+)?["'`]?([A-Za-z_]\w*)["'`]?\s*(\?)?\s*:""", p)
        if fm:
            fields.append((fm.group(1), bool(fm.group(2))))
            types[fm.group(1)] = p[fm.end() :].strip().rstrip(";,").strip()
    return fields, types


def object_literal_fields(t: str) -> list[tuple[str, bool]] | None:
    """An inline `{ a: X; b?: Y }` written straight into `invoke<..>`."""
    t = t.strip()
    if not (t.startswith("{") and t.endswith("}")):
        return None
    return _object_fields(t[1:-1])


def unwrap(t: str, aliases: dict[str, str] | None = None) -> tuple[str, bool, bool]:
    """`(Foo | null)[]` -> ("Foo", is_list=True, nullable=False). Peels null/undefined and arrays.

    A **tuple** (`[number, boolean]`) resolves to the sentinel `"<tuple>"` with `is_list=True`: it
    is a JSON array on the wire, which is exactly what a Rust tuple or fixed-size array serialises
    to, and calling it "a single value" made four correct call sites look broken.
    """
    aliases = aliases or {}
    t = t.strip()
    nullable = False
    is_list = False
    for _ in range(8):
        before = t
        if t.startswith("(") and _match(t, 0, "(", ")") == len(t) - 1:
            t = t[1:-1].strip()
        parts = [p.strip() for p in split_top(t, "|")]
        if len(parts) > 1:
            keep = [p for p in parts if p not in ("null", "undefined")]
            if len(keep) != len(parts):
                nullable = True
                t = " | ".join(keep) if keep else "null"
        if t.endswith("[]"):
            t, is_list = t[:-2].strip(), True
        am = re.fullmatch(r"(?:Readonly)?Array<(.*)>", t)
        if am:
            t, is_list = am.group(1).strip(), True
        if t.startswith("[") and _match(t, 0, "[", "]") == len(t) - 1:
            return f"<tuple:{len(split_top(t[1:-1]))}>", True, nullable
        if t in aliases:
            t = aliases[t]
        if t == before:
            break
    return t, is_list, nullable


def parse(sources: list[tuple[str, str]], call_files: set[str]) -> TsIpc:
    out = TsIpc()
    for rel, text in sources:
        src = strip_comments(text)
        if rel in call_files:
            out.invocations.extend(parse_invocations(src, rel))
        out.types.update(parse_types(src, rel))
        out.aliases.update(parse_aliases(src, rel))
    return out
