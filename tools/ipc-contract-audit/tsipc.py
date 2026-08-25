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
    #: key -> the TypeScript type DECLARED for the value at that key, for the one form where the
    #: declaration is unambiguous: a shorthand (`{ request }`) whose name is bound in an enclosing
    #: scope by an annotated parameter or `const`/`let`. An argument payload handed over inside one
    #: key is otherwise compared by its KEY alone — `author_rule({ rule })` agrees about the word
    #: "rule" and about nothing inside it — which is the whole subject of the `argfields` check.
    key_types: dict[str, str] = field(default_factory=dict)


@dataclass
class TsType:
    name: str
    fields: list[tuple[str, bool]]  # (key, optional)
    line: int
    file: str
    #: The names an `interface X extends A, B` inherits from, verbatim. Kept and resolved after every
    #: file is parsed, because a parent may be declared in another one. Dropping them was a live
    #: false-finding path on the ARGUMENT side and only a silence on the reply side: a reply check
    #: that misses inherited fields under-counts and says nothing, while `argfields` compares Rust
    #: REQUIRES against TypeScript DECLARES and turns the same under-count into a confident
    #: accusation that correct code fails to deserialise.
    extends: tuple[str, ...] = ()
    #: Another swept TypeScript file declares a different type by the same bare name. `ts.types` is
    #: keyed by bare name and the last file parsed wins — the same hazard `Dto.collides_with` records
    #: on the Rust side, and it was left unguarded on this side while the Rust side's docstring
    #: argued at length against exactly it. A collision makes the name unusable, not merely
    #: ambiguous: `argfields` refuses and counts a gap.
    collides_with: str = ""
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
    bindings = annotated_bindings(src)
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
        key_types: dict[str, str] = {}
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
                        # SHORTHAND ONLY. `{ request }` means the key and the variable are the same
                        # name, so a binding of that name is a statement about that key. `{ request:
                        # x }` is not: `x` would have to be followed, and `{ request: {..} }` is a
                        # literal whose keys are visible but whose spreads and conditionals are not.
                        # Both are left to the header's count rather than guessed at.
                        if kv == k:
                            ty = binding_type(bindings, k, i + 1)
                            if ty:
                                key_types[k] = ty
                    else:
                        spread = True  # computed key — the set is not fully known
            else:
                spread = True  # the argument object is a variable; its keys are not visible here
        out.append(Invocation(nm.group(1), targ, keys, line, rel, spread=spread, key_types=key_types))
    return out



#: A `const`/`let`/`var` carrying an explicit type annotation. `=` ends the type, and `[^=;]` keeps
#: the match inside one statement so a missing initialiser cannot swallow the rest of the file.
_ANNOTATED_LOCAL = re.compile(r"\b(?:const|let|var)\s+([A-Za-z_]\w*)\s*:\s*([^=;{]+?)\s*=")

#: The same declaration WITHOUT a type. It binds nothing this reader can compare, and it is recorded
#: for exactly that reason: a name that shadows must shadow whether or not it says what it holds.
#: `[^=]` after the `=` keeps it off `==` and `=>`.
_UNANNOTATED_LOCAL = re.compile(r"\b(?:const|let|var)\s+([A-Za-z_]\w*)\s*=[^=]")

#: One simple parameter: `name: T`, `name?: T`, `readonly name: T`. Destructuring (`{ a }: T`) and
#: rest (`...xs: T[]`) deliberately do NOT match — they bind something other than one name to that
#: type, and a reader that took the annotation anyway would attribute the wrong shape to the wrong
#: name. Those are left unresolved, which the header counts, rather than guessed at.
_SIMPLE_PARAM = re.compile(r"^(?:readonly\s+)?([A-Za-z_]\w*)\s*\??\s*:\s*(.+)$", re.S)


def _block_end(src: str, i: int) -> int:
    """The index of the `}` closing the innermost block open at `i`, or len(src)."""
    depth, n = 0, len(src)
    while i < n:
        c = src[i]
        if c in "\"'`":
            j = i + 1
            while j < n and src[j] != c:
                j += 2 if src[j] == "\\" else 1
            i = j + 1
            continue
        if c == "{":
            depth += 1
        elif c == "}":
            if depth == 0:
                return i
            depth -= 1
        i += 1
    return n


def annotated_bindings(src: str) -> list[tuple[str, str, int, int]]:
    """Every `name: T` binding whose TYPE is written down, with the span of source it governs.

    Two forms, both purely syntactic: a **function/method parameter**, whose scope is the body that
    follows its parameter list, and an annotated **local**, whose scope runs to the end of the block
    that holds it. Returned as `(name, type, start, end)` so a call site can ask for the binding
    whose span CONTAINS it and, where several do, the innermost — which is what shadowing means.

    Scope is the whole point, and the reason this is not a "nearest preceding declaration" scan. In

        setRule(rule: RuleData) { .. }
        author() { const rule = build(); invoke("author_rule", { rule }); }

    the nearest preceding annotation of `rule` is a parameter of a function the call is not in.
    Reading it would attribute `RuleData` to a value whose type is unknown and then compare a real
    struct against it — a confident wrong answer, which is the failure this tool exists to avoid and
    strictly worse than the silence it replaces. Here `author`'s `rule` has no annotation, no span
    contains the call, and the key resolves to nothing.
    """
    out: list[tuple[str, str, int, int]] = []
    n = len(src)

    # Parameters. Every `(` whose match is followed by an optional return annotation and then `{` is
    # a signature — a method, a function, or an arrow with a block body — and NESTED ones are visited
    # too. An earlier version resumed at the closing paren, so it only ever saw top-level
    # parentheses: a callback written inline, `xs.forEach((rule: RuleData) => { invoke(..) })`, is a
    # signature inside a call's argument list and produced no binding at all, while the comment above
    # it said "every `(`". That was honest silence rather than a wrong answer — the key stayed
    # unresolved and was counted — but it is silence over the shape a payload is most often built in,
    # and a comment that overstates what the code reaches is the thing this project treats as a
    # defect. Resuming one character later costs 2,353 `_match` calls on the largest file here.
    i = 0
    while i < n:
        c = src[i]
        if c in "\"'`":
            j = i + 1
            while j < n and src[j] != c:
                j += 2 if src[j] == "\\" else 1
            i = j + 1
            continue
        if c != "(":
            i += 1
            continue
        cp = _match(src, i, "(", ")")
        if cp < 0:
            break
        k = cp + 1
        while k < n and src[k].isspace():
            k += 1
        if src[k : k + 2] == "=>":                      # arrow: skip the fat arrow
            k += 2
            while k < n and src[k].isspace():
                k += 1
        elif src[k] == ":":                             # a return-type annotation before the body
            # `}` ENDS THE SEARCH, and leaving it out was a live false-finding path. An interface
            # member written without a trailing semicolon --
            #     interface Api { entityMark(request: EntityInfo): Promise<boolean> }
            # -- has no `;` to stop on, so the scan walked past the interface's own `}`, through the
            # next function's balanced parentheses, and adopted THAT function's body as the scope of
            # a parameter belonging to a type DECLARATION. The two bindings then had identical spans
            # and the tie went to insertion order, which put the phantom first. One semicolon made
            # six blocking findings appear and disappear.
            depth = 0
            while k < n:
                c2 = src[k]
                if c2 == "{" and depth == 0:
                    break
                if c2 in "<([":
                    depth += 1
                elif c2 in ">)]":
                    depth -= 1
                elif c2 in ";}" and depth == 0:
                    break                               # a declaration, not a definition
                k += 1
        if k >= n or src[k] != "{":
            i += 1
            continue
        body_end = _match(src, k, "{", "}")
        if body_end < 0:
            body_end = n
        for part in split_top(src[i + 1 : cp]):
            head = split_top(part, "=")[0].strip()
            pm = _SIMPLE_PARAM.match(head)
            if pm:
                out.append((pm.group(1), pm.group(2).strip(), k, body_end))
            elif re.fullmatch(r"(?:readonly\s+)?[A-Za-z_]\w*\??", head):
                # A parameter with NO annotation, recorded with an empty type so that it SHADOWS.
                out.append((head.replace("readonly", "").strip(" ?"), "", k, body_end))
        i += 1

    for m in _ANNOTATED_LOCAL.finditer(src):
        out.append((m.group(1), m.group(2).strip(), m.end(), _block_end(src, m.end())))
    for m in _UNANNOTATED_LOCAL.finditer(src):
        out.append((m.group(1), "", m.end(), _block_end(src, m.end())))
    return out


def binding_type(bindings: list[tuple[str, str, int, int]], name: str, at: int) -> str | None:
    """The declared type of `name` at offset `at` — from the INNERMOST span that contains it.

    An UNANNOTATED binding is carried in this list with an empty type, and it shadows exactly as a
    typed one does. Leaving it out was a live false-finding path with the same shape as the one the
    scope rule exists to prevent, one step further in:

        const request: EntityInfo = ..;                        // module scope, span runs to EOF
        entityMark() { const request = buildMarkRequest();     // shadows, and says no type
                       invoke("entity_mark", { request }); }

    The inner declaration is the one in effect and it declares nothing, so the honest answer is "no
    type", not the outer one. Reading the outer compared a real payload struct against an unrelated
    reply type and reported every field of both — which is precisely the wrong answer the
    `_binding_ignoring_scope` mutation is written to pin, still reachable through a hole beside it.

    Ties are broken by START offset, not by insertion order: two spans of equal length are two
    scopes, and the later one is the inner one.
    """
    hits = [b for b in bindings if b[0] == name and b[2] <= at <= b[3]]
    if not hits:
        return None
    return min(hits, key=lambda b: (b[3] - b[2], -b[2]))[1] or None


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
    for m in re.finditer(
        r"\bexport\s+interface\s+([A-Za-z_]\w*)(?:<[^>{]*>)?\s*(?:extends(?P<sup>[^{]+))?\{", src
    ):
        ob = src.index("{", m.end() - 1)
        cb = _match(src, ob, "{", "}")
        if cb < 0:
            continue
        fs, fts = _object_fields_typed(src[ob + 1 : cb])
        # Only a bare parent NAME is followed. `extends Pick<T, "a">` and other computed parents are
        # recorded verbatim and fail to resolve below, which marks the child unusable rather than
        # letting it claim to be complete — the whole point of keeping the clause.
        sup = tuple(x.strip() for x in split_top(m.group("sup") or "") if x.strip())
        types[m.group(1)] = TsType(
            m.group(1), fs, src.count("\n", 0, m.start()) + 1, rel, field_types=fts, extends=sup
        )
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


def string_union(t: str, aliases: dict[str, str] | None = None) -> list[str] | None:
    """The string literals a type expression admits, or None if it is not a union of them.

    `"a" | "b"`, a named alias of one, `("a" | "b")[]` and `Foo | null` all resolve; `string`,
    `"a" | number` and anything with a non-literal member do not — a union with `string` in it
    admits every string, so nothing about it is checkable and pretending otherwise would be the
    confident-answer-on-a-failed-read this tool exists to avoid.

    Order is preserved and duplicates are kept out, so a caller can report the members in the order
    a reader would find them in the source.
    """
    base, _is_list, _nullable = unwrap(t, aliases)
    base = base.strip()
    if base.startswith("(") and _match(base, 0, "(", ")") == len(base) - 1:
        base = base[1:-1].strip()
    parts = [p.strip() for p in split_top(base, "|")]
    if not parts or parts == [""]:
        return None
    out: list[str] = []
    for p in parts:
        if p in ("null", "undefined"):
            continue
        m = re.fullmatch(r"""["'](.*)["']""", p, re.S)
        if not m:
            return None
        if m.group(1) not in out:
            out.append(m.group(1))
    return out or None


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


#: How deep an `extends` chain is followed. Two links is already past everything this boundary
#: declares; the cap exists so a cycle cannot hang the gate, and a type still unresolved at the cap
#: is marked unusable rather than reported as complete.
_EXTENDS_DEPTH = 4


def _fold_extends(types: dict[str, TsType]) -> None:
    """Splice each parent's fields into the child, in place.

    A child REDECLARING a parent's key wins — that is what TypeScript does — so the parent's version
    is only added for keys the child does not state. A parent this reader cannot find (declared in a
    file the audit does not sweep, or computed with `Pick`/`Omit`) leaves the child marked
    `collides_with`, which `argfields` treats as unusable. Saying "I could not read this type" and
    "this type is complete and disagrees with the Rust" must never produce the same output.
    """
    for _ in range(_EXTENDS_DEPTH):
        moved = False
        for t in types.values():
            if not t.extends:
                continue
            unresolved = [p for p in t.extends if types.get(re.sub(r"<.*", "", p).strip()) is None]
            if unresolved:
                t.collides_with = t.collides_with or (
                    f"extends `{unresolved[0]}`, which is not a swept object type"
                )
                t.extends = ()
                moved = True
                continue
            if any(types[re.sub(r"<.*", "", p).strip()].extends for p in t.extends):
                continue  # a grandparent is still pending; settle it first
            own = {k for k, _ in t.fields}
            for p in t.extends:
                par = types[re.sub(r"<.*", "", p).strip()]
                if par.collides_with:
                    t.collides_with = t.collides_with or f"extends `{par.name}`, which is unusable"
                for k, opt in par.fields:
                    if k not in own:
                        t.fields.append((k, opt))
                        t.field_types.setdefault(k, par.field_types.get(k, ""))
                        own.add(k)
            t.extends = ()
            moved = True
        if not moved:
            break
    # A cycle (`A extends B`, `B extends A`) survives the loop with `extends` still set.
    for t in types.values():
        if t.extends:
            t.collides_with = t.collides_with or "its `extends` chain does not terminate"
            t.extends = ()


def parse(sources: list[tuple[str, str]], call_files: set[str]) -> TsIpc:
    out = TsIpc()
    for rel, text in sources:
        src = strip_comments(text)
        if rel in call_files:
            out.invocations.extend(parse_invocations(src, rel))
        here = parse_types(src, rel)
        for name, t in here.items():
            prior = out.types.get(name)
            if prior is not None and prior.file != rel and (
                [f for f, _ in prior.fields] != [f for f, _ in t.fields] or prior.extends != t.extends
            ):
                t.collides_with = f"{prior.file}:{prior.line} declares a different `{name}`"
            out.types[name] = t
        out.aliases.update(parse_aliases(src, rel))
    _fold_extends(out.types)
    return out
