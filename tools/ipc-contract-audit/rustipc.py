"""The Rust half of the IPC contract: what the shell says a command is.

Every `#[tauri::command]` is a promise made to code the Rust compiler will never see — a name, a set
of argument keys, and the JSON shape of a reply. Tauri resolves that promise by *string lookup at
run time*: an unknown command name is a rejected promise, a missing argument key is a deserialize
error, and a renamed reply field is `undefined` in JavaScript — which throws nothing at all and
renders as a blank.

This module recovers the Rust side of that promise so it can be compared at rest. It reads
declarations, not Rust:

  * `#[tauri::command]` functions — name, the arguments that genuinely come from JS, and the
    return type;
  * the `generate_handler![..]` list, which is what actually decides whether a name exists;
  * `#[derive(Serialize)]` structs, resolved to their WIRE field names under `rename_all`,
    `rename`, `skip` and `skip_serializing_if`;
  * a command that returns `serde_json::Value` by building one `serde_json::json!({..})` — the
    ad-hoc DTOs, which are the ones with no type anywhere to protect them.

Anything else is recorded as unresolved and reported. "I could not read it" is never allowed to
render as "it is fine" — that is the failure this gate exists to prevent.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from dataclasses import field as dc_field

# ── Arguments Tauri injects, which JS therefore never sends ───────────────────────────────────────
# Matched against the type's LAST PATH SEGMENT with its generics stripped, never as a substring.
# `metrocalk_core::StateMachine` contains the text "State" and is a perfectly ordinary argument;
# dropping it as an injected `State<_>` would silently remove a real key from the comparison.
INJECTED_TYPES = frozenset(
    {"State", "AppHandle", "Window", "WebviewWindow", "Webview", "App", "Request", "CommandItem"}
)

# NOT injected, deliberately: `Channel<T>`. Tauri constructs it on the JS side (`new Channel()`) and
# passes it through like any other argument, so it IS a key the caller must send.

_CMD_ATTR = re.compile(r"#\[tauri::command(?:\s*\((?P<args>[^)]*)\))?\s*\]")
_DERIVE = re.compile(r"#\[derive\(([^)]*)\)\]")
_RENAME_ALL = re.compile(r'rename_all\s*=\s*"([^"]+)"')
_RENAME = re.compile(r'\brename\s*=\s*"([^"]+)"')


@dataclass
class Command:
    name: str
    args: list[tuple[str, str, bool]]  # (wire key, rust type, optional)
    ret: str
    line: int
    file: str
    unreadable: str | None = None


@dataclass
class Dto:
    """A Rust type resolved to the JSON object it serialises to."""

    name: str
    fields: list[tuple[str, bool]]  # (wire key, may be absent from the payload)
    line: int
    file: str
    origin: str = "struct"  # "struct" | "json!" — how the shape was recovered
    #: Fields typed `serde_json::Value`. The key IS on the wire, so the presence check passes, but
    #: whatever the caller reads *out of* it is unchecked — the waived top-level defect, one level
    #: down, and counted as covered unless it is named here.
    opaque: tuple[str, ...] = ()
    #: Set when another file in the swept trees declares a different struct by the same bare name.
    #: `rs.dtos` is keyed by bare name, so the second one silently wins; a verdict derived from the
    #: wrong `Composition` would be confident and wrong.
    collides_with: str = ""
    #: wire key -> the Rust type expression behind it. Field NAMES alone are enough to compare one
    #: level; they are not enough to go a second, because the reader cannot follow a field it cannot
    #: name a type for. Kept beside `fields` rather than folded into it so every existing consumer of
    #: `(key, optional)` is untouched.
    field_types: dict[str, str] = dc_field(default_factory=dict)


@dataclass
class RustIpc:
    commands: dict[str, Command] = field(default_factory=dict)
    registered: list[str] = field(default_factory=list)
    dtos: dict[str, Dto] = field(default_factory=dict)
    handler_found: bool = False


# ── source hygiene ────────────────────────────────────────────────────────────────────────────────


def strip_line_comments(src: str) -> str:
    """Drop `//` to end of line, leaving the line count intact.

    Comments must go before anything is matched: this codebase documents its defects in prose
    directly above them, and a gate that reads the description of a bug as the bug reports the
    opposite of the truth. The quote count guards the `"http://"` case.
    """
    out = []
    for ln in src.splitlines():
        i = ln.find("//")
        while i >= 0:
            if ln.count('"', 0, i) % 2 == 0:
                ln = ln[:i]
                break
            i = ln.find("//", i + 1)
        out.append(ln)
    return "\n".join(out)


def _match(src: str, i: int, opens: str, closes: str) -> int:
    """Index of the delimiter closing the one at `i`, or -1. Skips string literals and chars."""
    depth = 0
    n = len(src)
    while i < n:
        c = src[i]
        if c == '"':
            i += 1
            while i < n and src[i] != '"':
                i += 2 if src[i] == "\\" else 1
        elif c in opens:
            depth += 1
        elif c in closes:
            depth -= 1
            if depth == 0:
                return i
        i += 1
    return -1


def _angle_is_bracket(src: str, i: int) -> bool:
    """Is the `<` or `>` at `i` a generic bracket, rather than half of an operator?

    Rust writes `=>` in every match arm and `->` in every signature. Counting those as brackets
    unbalances the depth, and the first comma after a match arm then reads as a top-level separator:
    `"setViewArg": match active { A => "x", B => "y" }` split into three entries, one of which is
    not a key-value pair, so `colour_status`'s whole reply was abandoned as unreadable. It was the
    largest hand-built DTO in the shell — the one with the most to gain from being compared.

    Note which way the `>>` case is decided: `collect::<Vec<_>>()` closes two generics, and Rust's
    own parser has the same ambiguity with a right shift. Nested generic closes are everywhere in
    this codebase and `a >> b` inside a reply literal is not, so `>>` counts as two brackets. When
    that guess is wrong the split simply fails and the reply is reported UNRESOLVED — an admitted
    unknown, never a confident wrong shape.
    """
    c = src[i]
    prev = src[i - 1] if i else ""
    nxt = src[i + 1] if i + 1 < len(src) else ""
    if c == ">":
        return prev not in "=-" and nxt != "="
    return nxt not in "=<" and prev not in "=<>"


def split_top(src: str, sep: str = ",") -> list[str]:
    """Split on `sep` at bracket depth 0, ignoring separators inside strings."""
    parts: list[str] = []
    depth = 0
    cur: list[str] = []
    i, n = 0, len(src)
    while i < n:
        c = src[i]
        if c == '"':
            j = i + 1
            while j < n and src[j] != '"':
                j += 2 if src[j] == "\\" else 1
            cur.append(src[i : j + 1])
            i = j + 1
            continue
        if c in "<>":
            if _angle_is_bracket(src, i):
                depth += 1 if c == "<" else -1
        elif c in "([{":
            depth += 1
        elif c in ")]}":
            depth -= 1
        if c == sep and depth == 0:
            parts.append("".join(cur))
            cur = []
        else:
            cur.append(c)
        i += 1
    if "".join(cur).strip():
        parts.append("".join(cur))
    return [p.strip() for p in parts if p.strip()]


def type_head(ty: str) -> str:
    """`tauri::State<'_, AppState>` -> `State`; `Option<Vec<T>>` -> `Option`; `[f32; 3]` -> `[]`."""
    ty = ty.strip()
    if ty.startswith("&"):
        ty = ty.lstrip("&").strip()
        ty = re.sub(r"^'\w+\s*", "", ty).strip()
        ty = re.sub(r"^mut\s+", "", ty).strip()
    if ty.startswith("["):
        return "[]"
    if ty.startswith("("):
        return "()" if ty.replace(" ", "") == "()" else "(,)"
    base = ty.split("<", 1)[0].strip()
    return base.split("::")[-1].strip()


def camel(s: str) -> str:
    head, *rest = s.split("_")
    return head + "".join(w[:1].upper() + w[1:] for w in rest)


def wire_case(name: str, rename_all: str | None) -> str:
    if rename_all == "camelCase":
        return camel(name)
    if rename_all == "PascalCase":
        c = camel(name)
        return c[:1].upper() + c[1:]
    if rename_all in ("SCREAMING_SNAKE_CASE", "SCREAMING-KEBAB-CASE"):
        out = name.upper()
        return out.replace("_", "-") if rename_all.endswith("KEBAB-CASE") else out
    if rename_all == "kebab-case":
        return name.replace("_", "-")
    if rename_all == "lowercase":
        return name.replace("_", "").lower()
    return name


# ── commands ──────────────────────────────────────────────────────────────────────────────────────


def _parse_commands(src: str, rel: str, out: RustIpc) -> None:
    for m in _CMD_ATTR.finditer(src):
        attr_args = m.group("args") or ""
        ra = _RENAME_ALL.search(attr_args)
        # Tauri's own default for command ARGUMENTS is camelCase on the JS side; `rename_all` on the
        # attribute overrides it (`snake_case` is the common override).
        rename_all = ra.group(1) if ra else "camelCase"

        tail = src[m.end() : m.end() + 600]
        fm = re.search(r"\bfn\s+([A-Za-z_]\w*)\s*\(", tail)
        line = src.count("\n", 0, m.start()) + 1
        if not fm:
            out.commands[f"<unreadable@{rel}:{line}>"] = Command(
                "", [], "", line, rel, unreadable="no `fn` follows this #[tauri::command]"
            )
            continue
        name = fm.group(1)
        op = m.end() + fm.end() - 1
        cp = _match(src, op, "(", ")")
        if cp < 0:
            out.commands[name] = Command(
                name, [], "", line, rel, unreadable="the argument list does not close"
            )
            continue

        args: list[tuple[str, str, bool]] = []
        for p in split_top(src[op + 1 : cp]):
            if ":" not in p:
                continue
            raw_name, raw_ty = p.split(":", 1)
            pname = re.sub(r"^mut\s+", "", raw_name.strip()).strip()
            ty = " ".join(raw_ty.split())
            if type_head(ty) in INJECTED_TYPES:
                continue
            args.append((wire_case(pname, rename_all), ty, type_head(ty) == "Option"))

        out.commands[name] = Command(name, args, _return_type(src, cp + 1), line, rel)


def _return_type(src: str, i: int) -> str:
    """The declared return type between `)` and the `{` that opens the body.

    Scanned rather than matched against `[^{;]+`: `-> [f64; 8]` contains a semicolon, and a regex
    that stopped there read *eight floats* as *no return value at all* — a confident wrong answer,
    which is worse than an unresolved one because nothing about it looks like a failed read.
    """
    m = re.match(r"\s*->\s*", src[i : i + 40])
    if not m:
        return "()"
    j = i + m.end()
    depth = 0
    while j < len(src):
        c = src[j]
        if c in "<([":
            depth += 1
        elif c in ">)]":
            depth -= 1
        elif (c == "{" or c == ";") and depth == 0:
            return " ".join(src[i + m.end() : j].split())
        j += 1
    return "()"


def _parse_handler(src: str, out: RustIpc) -> None:
    m = re.search(r"generate_handler!\s*\[", src)
    if not m:
        return
    ob = src.index("[", m.start() + len("generate_handler!") - 1)
    cb = _match(src, ob, "[", "]")
    if cb < 0:
        return
    out.handler_found = True
    for tok in split_top(src[ob + 1 : cb]):
        tok = tok.strip()
        if tok:
            out.registered.append(tok.split("::")[-1])


# ── serialised shapes ─────────────────────────────────────────────────────────────────────────────


def _is_opaque(ty: str) -> bool:
    """Does this field put a `serde_json::Value` on the wire, however it is wrapped?

    `Option<serde_json::Value>` and `Vec<serde_json::Value>` are just as shapeless as the bare form,
    and checking only the outermost head missed `TerrainReply.recipe` — one of the fields a panel
    reads a fully declared TypeScript interface out of.
    """
    for _ in range(4):
        head = type_head(ty)
        if head == "Value":
            return True
        if head in ("Option", "Vec", "VecDeque", "Box") and "<" in ty:
            ty = ty[ty.index("<") + 1 : ty.rindex(">")].strip()
            continue
        return False
    return False


def _parse_structs(src: str, rel: str, out: RustIpc) -> None:
    for m in _DERIVE.finditer(src):
        if "Serialize" not in m.group(1):
            continue
        tail = src[m.end() : m.end() + 800]
        sm = re.search(r"\b(?:pub(?:\([^)]*\))?\s+)?struct\s+([A-Za-z_]\w*)(?:<[^>{]*>)?\s*\{", tail)
        if not sm:
            continue
        attrs = tail[: sm.start()]
        # Another derive/struct/enum in between means this derive belongs to something else.
        if re.search(r"\b(struct|enum|fn|impl)\b", attrs):
            continue
        ra = _RENAME_ALL.search(attrs)
        rename_all = ra.group(1) if ra else None

        ob = m.end() + sm.end() - 1
        cb = _match(src, ob, "{", "}")
        if cb < 0:
            continue
        fields: list[tuple[str, bool]] = []
        opaque: list[str] = []
        ftypes: dict[str, str] = {}
        for p in split_top(src[ob + 1 : cb]):
            attr_lines = "\n".join(l for l in p.splitlines() if l.strip().startswith("#["))
            decl = "\n".join(l for l in p.splitlines() if not l.strip().startswith("#["))
            fm = re.match(r"\s*(?:pub(?:\([^)]*\))?\s+)?([A-Za-z_]\w*)\s*:\s*(.+)", decl, re.S)
            if not fm:
                continue
            fname = fm.group(1)
            fty = fm.group(2).replace("\n", " ").strip().rstrip(",").strip()
            is_opaque = _is_opaque(fty)
            serde_attr = " ".join(
                a for a in re.findall(r"#\[serde\(([^\]]*)\)\]", attr_lines)
            )
            if re.search(r"(^|[\s,(])skip(\s*[,)]|$)", serde_attr):
                continue
            if "flatten" in serde_attr:
                # A flattened field splices another type's keys in. Recording the field name would
                # invent a key that is never on the wire; recording nothing would invent agreement.
                fields.append((f"<flatten:{fname}>", True))
                continue
            rn = _RENAME.search(serde_attr)
            wire = rn.group(1) if rn else wire_case(fname, rename_all)
            fields.append((wire, "skip_serializing_if" in serde_attr))
            ftypes[wire] = fty
            if is_opaque:
                opaque.append(wire)
        name = sm.group(1)
        line = src.count("\n", 0, m.start()) + 1
        prior = out.dtos.get(name)
        collides = ""
        if prior is not None and [f for f, _ in prior.fields] != [f for f, _ in fields]:
            collides = f"{prior.file}:{prior.line}"
        out.dtos[name] = Dto(
            name, fields, line, rel, opaque=tuple(opaque), collides_with=collides, field_types=ftypes
        )


_JSON_MACRO = re.compile(r"\bserde_json::json!\s*\(\s*\{")


def _json_literal_keys(body: str) -> list[str] | None:
    """Top-level keys of the one `json!({..})` a command returns, or None if it is not that simple.

    Only a body whose *last* statement is a single top-level `json!({..})` is resolved. A command
    that builds its reply conditionally has more than one shape, and guessing which is the shape
    would be exactly the confident-diagnosis-on-top-of-a-failed-read that the GPU audit had to
    remove from its `attributes` message.
    """
    # The OUTERMOST macro whose block ends the function — not the last one to start. The replies
    # that matter build nested `json!`s inside `.map(|x| ..)` closures, and those start *after* the
    # outer macro does; taking the last hit read the innermost `{"id": .., "label": ..}` as the whole
    # reply, which is a confident wrong shape rather than an admitted unknown.
    for m in _JSON_MACRO.finditer(body):
        ob = body.index("{", m.end() - 1)
        cb = _match(body, ob, "{", "}")
        if cb < 0:
            continue
        # It must be the tail of the function: only whitespace / a trailing `)` after it.
        if re.sub(r"[\s);,]", "", body[cb + 1 :]) != "":
            continue
        keys: list[str] = []
        for entry in split_top(body[ob + 1 : cb]):
            km = re.match(r'^"([^"]*)"\s*:', entry.strip())
            if not km:
                return None  # a computed key, or a shape this reader does not understand
            keys.append(km.group(1))
        return keys or None
    return None


def attach_json_returns(src: str, rel: str, out: RustIpc) -> None:
    """Resolve `-> serde_json::Value` commands to the literal they build, where that is unambiguous."""
    for cmd in list(out.commands.values()):
        if cmd.file != rel or cmd.unreadable:
            continue
        if type_head(cmd.ret) not in ("Value",):
            continue
        fm = re.search(r"\bfn\s+" + re.escape(cmd.name) + r"\s*\(", src)
        if not fm:
            continue
        ob = src.find("{", src.index(")", fm.end() - 1))
        cb = _match(src, ob, "{", "}")
        if cb < 0:
            continue
        keys = _json_literal_keys(src[ob + 1 : cb])
        if keys is None:
            continue
        synth = f"json!@{cmd.name}"
        out.dtos[synth] = Dto(synth, [(k, False) for k in keys], cmd.line, rel, origin="json!")
        cmd.ret = synth


def parse(sources: list[tuple[str, str]]) -> RustIpc:
    """`sources` is [(relative path, text)]; later files may add DTOs the earlier ones return."""
    out = RustIpc()
    cleaned = [(rel, strip_line_comments(text)) for rel, text in sources]
    for rel, src in cleaned:
        _parse_commands(src, rel, out)
        _parse_handler(src, out)
        _parse_structs(src, rel, out)
    for rel, src in cleaned:
        attach_json_returns(src, rel, out)
    return out
