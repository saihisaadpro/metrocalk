"""The reply fields that untyped JavaScript reads — the `T` an E2E spec never writes down.

`invoke<T>("name")` in the editor's TypeScript states the reply shape once, in a declaration, and
`audit.py`'s `shape` check compares that declaration against the Rust. The 82 specs of the E2E suite
(146 files swept, the rest being wdio configs and page objects) state the same thing and never
declare it:

    const rules = await invoke("list_rules");
    if (rules[0].event !== "StateEntered") throw new Error("list_rules wrong");

`rules[0].event` **is** an assertion about the reply — written in usage rather than in a type. It is
exactly as binding as an `invoke<RuleSummary[]>` and exactly as capable of being wrong, and until
this module existed nothing anywhere compared it to the Rust. `tsc` never sees these files (plain
`.js`, no project, `checkJs` off); vitest never loads them; the `shape` check skips them because
`inv.targ is None`. The only thing that ever evaluated that line was a human, running wdio against a
packaged `.exe`, on a machine with a display.

That is not a hypothetical. On 2026-08-17 `RuleSummary` collapsed from a seven-field projection to
`{ id, rule }` — a strictly better DTO — and four specs went on reading the six fields that had
moved, one of them (`specs-hazard`, `r.name.includes(..)`) with a **TypeError**, one of them inside
M12.1's own live acceptance. Five gates were green. All five carried no information at all about
those files. They were found by an hour of adversarial reading, which is not a gate.

The lesson is the GPU audit's founding incident in another language. There it was
*a target that only ever gets compiled is not covered*; here it is its sibling — **a target that
only ever gets run by hand is not covered either**, and it rots more quietly, because between runs
nothing fails at all.

## What this module recovers

For every `invoke("cmd")` whose result is *bound* or *immediately read*, the field paths the code
subsequently reads off it:

  * `const x = await invoke("c"); x.a.b`            → `a.b`
  * `const xs = await invoke("c"); xs[0].a`         → `[].a`
  * `xs.map((r) => r.a)` / `.some` / `.every` / …   → `[].a`   (the callback's first parameter is
                                                                the element, in every method listed)
  * `for (const r of xs) r.a`                       → `[].a`
  * `(await invoke("c")).a`                         → `a`
  * `const [a, , b] = await invoke("c")`            → a positional claim of ARITY ≥ 3, compared
                                                      against the Rust tuple's length

A binding's scope runs from the call to the end of the enclosing block, and it ENDS EARLY at the
first thing that makes the name mean something else **at the same brace depth**: a reassignment
(`rules = ..`, `n += ..`), a re-declaration, or a nested function/arrow that takes the name as a
parameter. Depth matters: a rebind inside an `if` is a conditional retry, and truncating the outer
binding there dropped every read after the `if` from BOTH bindings. Reads are scanned over a copy of
the source in which every string and template literal has been blanked, so an assertion message
naming a field — the house style in this suite — is prose, not a read.

## What it deliberately does NOT claim

Silence here is not agreement, and the reach counters exist so a reader can tell the two apart. A
read is compared only when the Rust reply resolves to a **struct** whose keys are fully enumerable:

  * a `serde(flatten)` field splices in keys this reader cannot name, so a struct carrying one is
    **never** used to call a field absent — flatten makes absence unprovable, not false. (No read
    recovered from the real tree currently reaches the repository's one flattened struct, so this is
    today a property of the design and of the fixture, not a load-bearing filter);
  * a `serde_json::Value` reply (or a `Value` field) has no field names on the Rust side at all;
  * a `json!({..})`-derived shape is enumerable and IS used — that is `origin == "json!"`, already
    resolved by `rustipc`;
  * every JavaScript built-in (`length`, `map`, `includes`, `then`, …) is a read of the *language*,
    not of the reply. The cost of that exclusion is stated rather than hidden: a Rust reply field
    genuinely named `length` or `filter` would not be checked here.

The counters (`paths`, `steps`, `unresolved`) are reported in the header for the same reason the
Rust sweep counts its files: a check that quietly stops reaching things looks identical to a check
that keeps passing.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field

#: Properties that belong to JavaScript, not to a reply. A read of one of these says nothing about
#: the contract, and reporting it would bury the real findings under thousands of `.length`s.
#:
#: The exclusion is not free and the docstring says so: a reply field genuinely named `length`,
#: `filter` or `name` on a *function-valued* read would be skipped. `name` is NOT in this list —
#: `Function.prototype.name` exists, but a reply field called `name` is far commoner in this
#: codebase than a function read out of a reply, and `r.name` on a `RuleSummary` is precisely the
#: read that crashed `specs-hazard`. Getting that one wrong in the safe direction would have made
#: this module miss the defect it was written for.
JS_BUILTINS = frozenset(
    {
        # Array
        "length", "map", "filter", "find", "findIndex", "findLast", "findLastIndex", "some", "every",
        "forEach", "flatMap", "flat", "reduce", "reduceRight", "slice", "splice", "concat", "join",
        "includes", "indexOf", "lastIndexOf", "push", "pop", "shift", "unshift", "sort", "reverse",
        "at", "fill", "keys", "values", "entries", "copyWithin",
        # String
        "charAt", "charCodeAt", "codePointAt", "endsWith", "match", "matchAll", "normalize",
        "padEnd", "padStart", "repeat", "replace", "replaceAll", "search", "split", "startsWith",
        "substring", "substr", "toLowerCase", "toUpperCase", "trim", "trimStart", "trimEnd",
        # Object / Function / Promise
        "then", "catch", "finally", "constructor", "hasOwnProperty", "isPrototypeOf",
        "propertyIsEnumerable", "toString", "toLocaleString", "valueOf", "call", "apply", "bind",
        "prototype", "__proto__",
        # Number
        "toFixed", "toPrecision", "toExponential",
    }
)

#: Array methods whose callback takes the ELEMENT as its first parameter. `reduce` is absent on
#: purpose: its first parameter is the accumulator, and treating it as an element would attribute
#: reads to the wrong shape — a false finding, which is worse here than a missed one.
ELEMENT_METHODS = frozenset(
    {"map", "filter", "find", "findIndex", "findLast", "findLastIndex", "some", "every",
     "forEach", "flatMap"}
)

_IDENT = r"[A-Za-z_$][A-Za-z0-9_$]*"


@dataclass
class Read:
    """One field path read off one command's reply."""

    cmd: str
    path: list[str]  # steps; "[]" is "an element of the list", any other entry is a field name
    line: int
    file: str
    via: str = ""  # how the binding was recovered, for the finding's prose


@dataclass
class TupleRead:
    """`const [a, , b] = await invoke("c")` — a claim that the reply has at least 3 positions.

    An unrecognised BINDING FORM is the one reach hole no counter can show, because a call site that
    binds in a shape the reader does not parse increments nothing: it looks exactly like a call site
    that binds nothing. 16 real sites destructure a tuple reply positionally, and the arity they
    claim is as much a part of the contract as a field name — this tool already learned that once,
    on the typed side, where a Rust 5-tuple and a TS 3-tuple compared equal.
    """

    cmd: str
    positions: int  # how many positions the pattern claims exist
    line: int
    file: str
    text: str = ""


@dataclass
class JsReads:
    reads: list[Read] = field(default_factory=list)
    tuples: list[TupleRead] = field(default_factory=list)
    #: Call sites that bound a reply to a name this reader then found no reads for. Not a defect —
    #: `await invoke("undo")` binds nothing and reads nothing — but counted, because a parser change
    #: that silently stopped finding reads would look exactly like a clean tree.
    bound: int = 0
    #: Sub-paths of a reply given a NAME of their own (`const xs = reply.items ?? [];`) and then
    #: followed through that name. Counted separately because it is the reach that was missing: the
    #: reads it recovers were invisible for as long as this reader tracked only the bound reply
    #: itself, and a run that followed none and a run with no aliases to follow print the same
    #: findings. See `_reads_of`'s alias branch for what is deliberately NOT followed.
    aliases: int = 0
    #: Occurrences of a tracked name that were NOT attributed, because a function between the
    #: binding and the read takes that name as a parameter. Counted rather than silently skipped:
    #: this is the one number that separates "the shadow rule found nothing to do" from "the shadow
    #: rule stopped working", and the two are otherwise the same output.
    shadowed: int = 0


# ── source scanning ───────────────────────────────────────────────────────────────────────────────


def _skip_string(src: str, i: int) -> int:
    """Index just past the string/template literal opening at `i`."""
    q = src[i]
    j = i + 1
    n = len(src)
    while j < n:
        c = src[j]
        if c == "\\":
            j += 2
            continue
        if c == q:
            return j + 1
        if q == "`" and c == "$" and j + 1 < n and src[j + 1] == "{":
            depth = 1
            j += 2
            while j < n and depth:
                if src[j] in "\"'`":
                    j = _skip_string(src, j)
                    continue
                if src[j] == "{":
                    depth += 1
                elif src[j] == "}":
                    depth -= 1
                j += 1
            continue
        j += 1
    return n


def blank_literals(src: str) -> str:
    """The source with every string and template literal's CONTENT replaced by spaces, positions and
    line count preserved.

    Reads are scanned over this, never over the raw text. This suite's house style is an assertion
    message that names the field it is about — `throw new Error("expected info.jointKindLabel to be
    set")` — and a reader that does not blank literals recovers `jointKindLabel` as a real read of
    the reply and reports the reply for not sending it. Prose is not a claim about the wire.
    """
    out: list[str] = []
    i, n = 0, len(src)
    while i < n:
        c = src[i]
        if c in "\"'`":
            j = _skip_string(src, i)
            body = src[i:j]
            out.append(c + "".join(" " if ch != "\n" else "\n" for ch in body[1:-1]) + body[-1:]
                       if len(body) >= 2 else body)
            i = j
            continue
        out.append(c)
        i += 1
    blanked = "".join(out)
    assert len(blanked) == len(src), "blanking must preserve positions"
    return blanked


def block_end(src: str, i: int) -> int:
    """The index at which the block enclosing `i` closes.

    Walked forward counting braces: the position where depth would go negative is where the
    enclosing block ends, which is exactly the end of the binding's scope. Cheaper and more robust
    than finding the opening brace by walking backwards, and it needs no knowledge of what kind of
    block it is (function, `it(..)`, `if`, arrow body — all the same rule).
    """
    depth = 0
    n = len(src)
    while i < n:
        c = src[i]
        if c in "\"'`":
            i = _skip_string(src, i)
            continue
        if c == "{":
            depth += 1
        elif c == "}":
            if depth == 0:
                return i
            depth -= 1
        i += 1
    return n


#: `const xs = ` immediately before a tracked name — the alias form. Anchored by the caller against
#: the exact start of the name it matched, so `const ys = other.xs` cannot be mistaken for it.
_ALIAS_OF = re.compile(rf"(?:const|let|var)\s+({_IDENT})\s*=\s*")

#: The only tail an alias initializer may carry and still hold the sub-path's shape. `?? []` and
#: `?? {}` are the suite's idiom for "or nothing"; anything else — another expression, a ternary, an
#: arithmetic operator — changes what the name holds, and following it would attribute a foreign
#: shape's reads to this reply.
_EMPTY_DEFAULT = re.compile(r"(?:\?\?|\|\|)\s*(?:\[\s*\]|\{\s*\})")


def _statement_end(src: str, i: int, limit: int) -> int | None:
    """The `;` ending the statement that is still open at `i`, or `None` if it does not simply end.

    Depth-aware, so the brackets of a `?? []` default and the parens of anything else are stepped
    over rather than mistaken for the end. Returning `None` when the statement does not close with a
    plain `;` before `limit` — or when depth goes negative, meaning `i` was never inside a statement
    at all — is the conservative half of the alias rule: an initializer this cannot delimit is one
    whose tail cannot be checked, and an unchecked tail is not followed.
    """
    depth = 0
    while i < limit and i < len(src):
        c = src[i]
        if c in "\"'`":
            i = _skip_string(src, i)
            continue
        if c in "([{":
            depth += 1
        elif c in ")]}":
            depth -= 1
            if depth < 0:
                return None
        elif c == ";" and depth == 0:
            return i
        i += 1
    return None


def _chain(src: str, i: int) -> tuple[list[str], int]:
    """Read the `.a`, `?.b`, `[expr]` steps starting at `i`. Returns (steps, index past them).

    `[expr]` becomes `"[]"` — "an element of" — whatever the expression is. Indexing a list by a
    literal, a variable or a computed expression are the same claim about the shape.
    """
    steps: list[str] = []
    n = len(src)
    while i < n:
        j = i
        while j < n and src[j] in " \t\n\r":
            j += 1
        if src.startswith("?.", j):
            j += 2
        elif j < n and src[j] == ".":
            j += 1
        elif j < n and src[j] == "[":
            k, depth = j, 0
            while k < n:
                if src[k] in "\"'`":
                    k = _skip_string(src, k)
                    continue
                if src[k] == "[":
                    depth += 1
                elif src[k] == "]":
                    depth -= 1
                    if depth == 0:
                        break
                k += 1
            if k >= n:
                break
            steps.append("[]")
            i = k + 1
            continue
        else:
            break
        m = re.match(_IDENT, src[j:])
        if not m:
            break
        steps.append(m.group(0))
        i = j + m.end()
    return steps, i


def _reads_of(src: str, start: int, end: int, name: str, prefix: list[str], out: list[Read],
              line_of, rel: str, cmd: str, via: str, depth: int = 0,
              tally: "JsReads | None" = None) -> None:
    """Every read of `name` in `src[start:end]`, recorded with `prefix` in front of its path.

    Recurses once per element-yielding callback (`xs.map((r) => r.a)`), which is how a read reached
    through an alias keeps its `[]` step — the thing that makes `rules.map(r => r.name)` and
    `rules[0].name` the same claim, as they are.

    …and once per ALIAS BINDING (`const xs = reply.items ?? []; xs.filter((x) => x.flag)`), which is
    the same idea one level out and was the reader's blind spot until 2026-08-25. Proved rather than
    supposed, on the committed tree: renaming the reply's top-level `shotPlacements` key was caught,
    and renaming `asDirected` INSIDE its elements — read through exactly that alias — was `0
    blocking`. The failure is silent in the worst way, because the spec does not throw: the E2E's
    `placements.filter((p) => !p.asDirected)` turns every element into a re-aimed one when the key
    goes missing (`!undefined` is `true`), so the run passes and its manifest reports a false number
    about the film.
    """
    if depth > 3:
        return
    # Where `name` means a parameter rather than this reply. Computed once for the range, because
    # every occurrence inside one of these spans is a read of somebody else's object.
    shadows = shadow_spans(src, name, start, end)
    for m in re.finditer(rf"(?<![A-Za-z0-9_$.]){re.escape(name)}(?![A-Za-z0-9_$])", src[start:end]):
        at = start + m.end()
        if any(a <= at - len(name) < b for a, b in shadows):
            if tally is not None:
                tally.shadowed += 1
            continue
        steps, after = _chain(src, at)
        if not steps:
            continue

        # `xs.map((r) => ..)` — the callback's parameter is an element, and its body's reads belong
        # to the element shape, not to the list.
        if len(steps) == 1 and steps[0] in ELEMENT_METHODS:
            k = after
            while k < len(src) and src[k] in " \t\n\r":
                k += 1
            if k < len(src) and src[k] == "(":
                param, body_start = _callback_param(src, k)
                if param:
                    body_end = _call_end(src, k)
                    _reads_of(src, body_start, body_end, param, prefix + ["[]"], out, line_of,
                              rel, cmd, via, depth + 1, tally)
            continue

        # Trailing builtins end the path: `x.a.length` is a read of `a`, then of the language.
        path: list[str] = []
        for s in steps:
            if s != "[]" and s in JS_BUILTINS:
                break
            path.append(s)
        if not path or all(s == "[]" for s in path):
            continue
        out.append(Read(cmd, prefix + path, line_of(at), rel, via))

        # `const xs = reply.items ?? [];` — this read is also a BINDING, and everything read off the
        # new name is a read of `items`. Followed under three conditions, each of which exists to
        # keep the alias's shape the same as the sub-path's:
        #
        #   * the chain reached the end of the initializer, so `const n = r.items.length;` and
        #     `const s = r.items.join(",");` do not qualify — `path` stopped at a builtin while
        #     `steps` did not, and what `n` holds is a number;
        #   * nothing follows it but an EMPTY default. `?? []` and `?? {}` are the house idiom for
        #     "or nothing" and leave the shape intact; `?? somethingElse`, `+ 1`, a ternary or a call
        #     do not, and following those would attribute a foreign shape's reads to this reply —
        #     a FALSE finding, which is worse here than a missed one (the same reasoning that keeps
        #     `reduce` out of ELEMENT_METHODS);
        #   * the alias is a fresh `const`/`let`/`var`. A bare reassignment is not followed: its
        #     prior meaning is not this reader's to reason about.
        if len(path) != len(steps):
            continue
        am = _ALIAS_OF.search(src, max(start, at - len(name) - 40), at - len(name))
        if not am or am.end() != at - len(name):
            continue
        rest_end = _statement_end(src, after, end)
        if rest_end is None:
            continue
        rest = src[after:rest_end].strip()
        if rest and not _EMPTY_DEFAULT.fullmatch(rest):
            continue
        alias = am.group(1)
        if tally is not None:
            tally.aliases += 1
        alias_start = rest_end + 1
        alias_end = _scope_end(src, alias, alias_start, min(end, block_end(src, alias_start)))
        _reads_of(src, alias_start, alias_end, alias, prefix + path, out, line_of, rel, cmd, via,
                  depth + 1, tally)


def _pattern_binds(params: str) -> set[str]:
    """Every name a parameter list BINDS — which is not every name it contains.

    `({ profile })` binds `profile`; `({ profile: p })` binds `p` and leaves `profile` meaning
    whatever it meant outside. Getting that backwards would suppress a legitimate read, so the
    destructuring is walked rather than grepped: only the position a value can be written to counts.
    """
    names: set[str] = set()
    for part in _split_top(params):
        part = re.split(r"(?<![=!<>])=(?!=)", part, maxsplit=1)[0].strip()  # drop a default
        part = re.sub(r"^\.\.\.", "", part).strip()  # a rest element binds its own name
        if not part:
            continue
        if part[0] in "{[":
            close = _match_bracket(part, 0)
            if close < 0:
                continue
            if part[0] == "{":
                for entry in _split_top(part[1:close]):
                    entry = re.split(r"(?<![=!<>])=(?!=)", entry, maxsplit=1)[0].strip()
                    entry = re.sub(r"^\.\.\.", "", entry).strip()
                    # `key: target` binds TARGET; a bare `key` binds the key itself.
                    names |= _pattern_binds(entry.split(":", 1)[1] if ":" in entry else entry)
            else:
                for entry in _split_top(part[1:close]):
                    names |= _pattern_binds(entry)
            continue
        m = re.match(rf"^{_IDENT}$", part)
        if m:
            names.add(part)
    return names


def _split_top(src: str) -> list[str]:
    """Split on commas at bracket depth 0."""
    parts, cur, depth, i, n = [], [], 0, 0, len(src)
    while i < n:
        c = src[i]
        if c in "\"'`":
            j = _skip_string(src, i)
            cur.append(src[i:j])
            i = j
            continue
        if c in "([{":
            depth += 1
        elif c in ")]}":
            depth -= 1
        if c == "," and depth == 0:
            parts.append("".join(cur))
            cur = []
        else:
            cur.append(c)
        i += 1
    parts.append("".join(cur))
    return [p.strip() for p in parts if p.strip()]


def _match_bracket(src: str, i: int) -> int:
    """Index of the bracket closing the one at `i`, or -1."""
    opens, depth, n = "([{", 0, len(src)
    while i < n:
        c = src[i]
        if c in "\"'`":
            i = _skip_string(src, i)
            continue
        if c in opens:
            depth += 1
        elif c in ")]}":
            depth -= 1
            if depth == 0:
                return i
        i += 1
    return -1


def shadow_spans(src: str, name: str, start: int, end: int) -> list[tuple[int, int]]:
    """The body spans within `[start, end)` where `name` means a PARAMETER, not the reply.

    `_scope_end` already knows that a function taking the name shadows it, and checks it only at the
    binding's own brace depth — deliberately, because a rebind inside an `if` is a conditional retry
    and truncating there dropped the reads after it from both bindings. But a *shadow* is not an end
    to the scope at all: it is a HOLE in it, and one brace deeper the hole was invisible. On the real
    tree that misread `manifest.animation = { revolvingMechanisms:
    tracks.filter(({ profile }) => profile.cycle === "revolve").length }` as a read of the
    `set_render_profile` reply bound to `profile` eighty lines above — `set_render_profile` returns a
    `String`, so the walk stopped on "not a struct" and nothing was printed. It would have printed
    the day that command grew a struct reply: a confident finding, naming a real file and a real
    line, about code that is correct. This reader's whole authority rests on not doing that.

    Only forms whose parameter list is unambiguous are recognised — an arrow's, and a `function`'s.
    A method shorthand (`foo(bar) { .. }` in an object literal) is indistinguishable from a call
    followed by a block without a real parser, so it is left alone: missing a shadow leaves today's
    behaviour, and today's behaviour is the thing being improved, not a regression.
    """
    spans: list[tuple[int, int]] = []
    bound = re.compile(rf"(?<![A-Za-z0-9_$.]){re.escape(name)}(?![A-Za-z0-9_$])")

    def body_at(k: int) -> tuple[int, int] | None:
        while k < len(src) and src[k] in " \t\n\r":
            k += 1
        if k >= len(src):
            return None
        if src[k] == "{":
            return (k, block_end(src, k + 1))
        # A concise arrow body ends at the first separator that closes the expression it is in.
        j, depth = k, 0
        while j < len(src):
            c = src[j]
            if c in "\"'`":
                j = _skip_string(src, j)
                continue
            if c in "([{":
                depth += 1
            elif c in ")]}":
                if depth == 0:
                    break
                depth -= 1
            elif c in ",;" and depth == 0:
                break
            j += 1
        return (k, j)

    for m in re.finditer(r"=>", src[start:end]):
        at = start + m.start()
        i = at - 1
        while i >= start and src[i] in " \t\n\r":
            i -= 1
        if i < start:
            continue
        if src[i] == ")":
            j = i
            depth = 0
            while j >= start:  # walk back to the `(` this `)` closes
                if src[j] == ")":
                    depth += 1
                elif src[j] == "(":
                    depth -= 1
                    if depth == 0:
                        break
                j -= 1
            if j < start:
                continue
            params = src[j + 1 : i]
        else:  # `name => ..`, a single parameter with no parentheses
            j = i
            while j >= start and (src[j].isalnum() or src[j] in "_$"):
                j -= 1
            params = src[j + 1 : i + 1]
        if not bound.search(params) or name not in _pattern_binds(params):
            continue
        span = body_at(at + 2)
        if span:
            spans.append(span)

    for m in re.finditer(rf"\bfunction\b\s*\*?\s*{_IDENT}?\s*\(", src[start:end]):
        op = start + m.end() - 1
        cp = _call_end(src, op)
        params = src[op + 1 : cp]
        if not bound.search(params) or name not in _pattern_binds(params):
            continue
        span = body_at(cp + 1)
        if span:
            spans.append(span)
    return spans


def _callback_param(src: str, open_paren: int) -> tuple[str, int]:
    """The first parameter name of the callback in the call starting at `open_paren`, and where its
    body begins. Handles `(r) => ..`, `r => ..` and `function (r) { .. }`; anything else yields no
    parameter, and the callback's reads are simply not attributed."""
    end = _call_end(src, open_paren)
    inner = src[open_paren + 1 : end]
    m = re.match(rf"\s*\(\s*({_IDENT})\s*[,)]", inner)
    if not m:
        m = re.match(rf"\s*({_IDENT})\s*=>", inner)
    if not m:
        m = re.match(rf"\s*(?:async\s+)?function\s*\*?\s*{_IDENT}?\s*\(\s*({_IDENT})\s*[,)]", inner)
    if not m:
        return "", open_paren
    return m.group(1), open_paren + 1 + m.end()


def _is_grouped(src: str, invoke_at: int) -> bool:
    """Is `await invoke(..)` wrapped in a GROUPING paren — `(await invoke(..)).field` — rather than
    handed to a function — `expect(await invoke(..)).toBeTruthy()`?

    The two are indistinguishable from the closing paren alone, and reading them as the same thing
    is a false finding: `expect(await invoke("wallet_info")).toBeTruthy()` was reported as the reply
    failing to send `toBeTruthy`. A matcher blocklist would have papered over that one case; the
    question is structural, so the answer is too — walk back to the `(` this expression opens
    inside, and look at what precedes it. An identifier, `)` or `]` before it means it is a call or
    an index, and the chain after the close belongs to the caller's result, not to the reply.
    """
    i = invoke_at - 1
    while i >= 0 and src[i] in " \t\n\r":
        i -= 1
    if i >= 4 and src[i - 4 : i + 1] == "await":
        i -= 5
        while i >= 0 and src[i] in " \t\n\r":
            i -= 1
    if i < 0 or src[i] != "(":
        return False
    i -= 1
    while i >= 0 and src[i] in " \t\n\r":
        i -= 1
    return i < 0 or not (src[i].isalnum() or src[i] in "_$)]")


def _call_end(src: str, open_paren: int) -> int:
    depth, i, n = 0, open_paren, len(src)
    while i < n:
        c = src[i]
        if c in "\"'`":
            i = _skip_string(src, i)
            continue
        if c == "(":
            depth += 1
        elif c == ")":
            depth -= 1
            if depth == 0:
                return i
        i += 1
    return n


_INVOKE_CALL = re.compile(rf'(?:\.\s*|\b)invoke\s*(?:<[^>()]*>\s*)?\(\s*"([a-zA-Z0-9_]+)"')
_BINDING = re.compile(rf"(?:const|let|var)\s+({_IDENT})\s*=\s*(?:await\s+)?$")
_ASSIGN = re.compile(rf"(?<![A-Za-z0-9_$.])({_IDENT})\s*=\s*(?:await\s+)?$")
#: `const [a, , b] = await invoke(..)` — the positional binding form. Captured whole so the arity
#: can be counted from the pattern rather than guessed.
_DESTRUCTURE = re.compile(r"(?:const|let|var)\s*\[([^\]]*)\]\s*=\s*(?:await\s+)?$")
_FOR_OF = re.compile(rf"for\s*\(\s*(?:const|let|var)\s+({_IDENT})\s+of\s+({_IDENT})\s*\)")


def _scope_end(src: str, name: str, start: int, end: int) -> int:
    """Where `name` stops meaning the reply, within `[start, end)`.

    Three things end it, and all three are checked only at the SAME BRACE DEPTH as the binding:

      * an assignment of any kind (`x = ..`, `x += ..`) — but not `x => ..`, whose `=` is part of an
        arrow and used to truncate `const doc = await invoke(..); [1,2].map(doc => ..)` down to zero
        reads;
      * a re-declaration (`const x = ..` again);
      * a nested function or arrow that takes `name` as its FIRST parameter, which shadows it — this
        is what attributed `function describe(info) { return info.helperField }` to a reply bound to
        `info` two lines above.

    Depth is the part that took a second attempt. A rebind inside an `if` is a conditional retry
    (`let info = await invoke(..); if (retry) { info = await invoke(..); } if (!info.kind) ..`), and
    truncating the outer binding at it left the trailing reads outside BOTH bindings' scopes — two
    bindings counted, no reads recovered, and the header reporting reach that did not exist.
    """
    pat = re.compile(
        rf"(?<![A-Za-z0-9_$.]){re.escape(name)}\s*(?:[-+*/%|&^]|\*\*|<<|>>>?|\?\?|\|\||&&)?=(?![=>])"
        rf"|(?:const|let|var)\s+{re.escape(name)}(?![A-Za-z0-9_$])"
        rf"|function\s*\*?\s*{_IDENT}?\s*\(\s*{re.escape(name)}\s*[,)]"
        rf"|\(\s*{re.escape(name)}\s*\)\s*=>"
        rf"|(?<![A-Za-z0-9_$.]){re.escape(name)}\s*=>"
    )
    depth = 0
    i = start
    while i < end:
        c = src[i]
        if c == "{":
            depth += 1
        elif c == "}":
            depth -= 1
        elif depth == 0:
            m = pat.match(src, i)
            if m:
                return i
        i += 1
    return end


def parse(src: str, rel: str) -> JsReads:
    """Every reply-field read this file makes, attributed to the command that replied.

    `src` is expected COMMENT-STRIPPED; literals are blanked here, because the command names in
    `invoke("name")` must survive long enough to be read and everything after that is scanned over
    the blanked copy.
    """
    out = JsReads()
    scan = blank_literals(src)
    newlines = [i for i, c in enumerate(src) if c == "\n"]

    def line_of(pos: int) -> int:
        lo, hi = 0, len(newlines)
        while lo < hi:
            mid = (lo + hi) // 2
            if newlines[mid] < pos:
                lo = mid + 1
            else:
                hi = mid
        return lo + 1

    for m in _INVOKE_CALL.finditer(src):
        cmd = m.group(1)
        call_end = _call_end(src, src.index("(", m.start()))
        head = src[max(0, m.start() - 120) : m.start()]

        # (a) `(await invoke("c")).a` — read straight off the call, no name involved.
        j = call_end + 1
        while j < len(scan) and scan[j] in " \t\n\r":
            j += 1
        if j < len(scan) and scan[j] == ")" and _is_grouped(scan, m.start()):
            steps, _ = _chain(scan, j + 1)
            path = []
            for s in steps:
                if s != "[]" and s in JS_BUILTINS:
                    break
                path.append(s)
            if path and not all(s == "[]" for s in path):
                out.reads.append(Read(cmd, path, line_of(m.start()), rel, "read off the call"))
                out.bound += 1
                continue

        # (b) `const [a, , b] = await invoke("c")` — a positional claim, not a field one.
        dm = _DESTRUCTURE.search(head)
        if dm:
            parts = dm.group(1).split(",")
            positions = 0
            for idx, part in enumerate(parts):
                if part.strip().startswith("..."):
                    break  # a rest element claims nothing beyond the positions before it
                positions = idx + 1
            out.bound += 1
            if positions:
                out.tuples.append(TupleRead(cmd, positions, line_of(m.start()), rel,
                                            f"[{dm.group(1).strip()}]"))
            continue

        # (c) a named binding — `const x = await invoke("c")` or a bare reassignment `x = ..`.
        bm = _BINDING.search(head) or _ASSIGN.search(head)
        if not bm:
            continue
        name = bm.group(1)
        out.bound += 1
        scope_start = call_end + 1
        scope_end = _scope_end(scan, name, scope_start, block_end(scan, scope_start))

        _reads_of(scan, scope_start, scope_end, name, [], out.reads, line_of, rel, cmd, f"`{name}`",
                  tally=out)

        # (d) `for (const r of xs) r.a` — the loop variable is an element of the binding. The body's
        # scope starts at its own `{`, NOT at the `)`: counted from the `)`, the loop's own braces
        # read as a nested pair and `block_end` ran on to the ENCLOSING block's `}`, so reads from
        # an unrelated later loop over an unrelated later binding were attributed to this reply.
        for fm in _FOR_OF.finditer(scan, scope_start, scope_end):
            if fm.group(2) != name:
                continue
            body = fm.end()
            while body < len(scan) and scan[body] in " \t\n\r":
                body += 1
            if body < len(scan) and scan[body] == "{":
                body += 1
            _reads_of(scan, body, block_end(scan, body), fm.group(1), ["[]"], out.reads, line_of,
                      rel, cmd, f"`{name}`", tally=out)

    # One defect, one finding. The pre-fix `if (r.error) throw new Error(`… ${r.error}`)` reads the
    # same field twice on one line and produced the identical finding twice, which inflates
    # "blocking" with no added information.
    seen: set[tuple] = set()
    deduped: list[Read] = []
    for rd in out.reads:
        key = (rd.file, rd.line, rd.cmd, tuple(rd.path))
        if key in seen:
            continue
        seen.add(key)
        deduped.append(rd)
    out.reads = deduped
    return out
