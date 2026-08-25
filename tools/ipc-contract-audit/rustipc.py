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
  * `#[derive(Serialize)]` **enums**, resolved to the set of STRINGS they can put on the wire —
    which is a contract exactly as binding as a field name, and quieter when it drifts: a renamed
    field reads `undefined` and a blank renders, but a renamed *variant* arrives as a perfectly
    ordinary, non-null, correctly-typed string that simply never equals what the UI compares it to.
    Nothing is undefined, nothing throws, and the branch is false forever;
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
class SerdeEnum:
    """A Rust enum resolved to the set of strings it can put on the wire.

    Only a **unit-only** enum is a string on the wire. The moment one variant carries data, serde's
    default representation is externally tagged — `{"Variant": payload}`, an object — and comparing
    that to a string union would be a confident wrong answer. `string_like` records which it is and
    `why_not` records the reason, because "this enum is not comparable" and "this enum agrees" must
    never produce the same silence.
    """

    name: str
    variants: tuple[str, ...]  # WIRE names, in declaration order
    line: int
    file: str
    rename_all: str | None = None
    string_like: bool = True
    why_not: str = ""
    #: `#[serde(other)]` on a variant makes the enum accept any unknown string on the way IN, so a
    #: member the Rust does not declare is not provably dead. Absence stops being provable, exactly
    #: as `serde(flatten)` stops absence being provable for a struct.
    has_other: bool = False
    #: A struct of the same bare name exists. Both readers key by name, so a field typed `Action`
    #: could mean either; the struct reader is the older and better-guarded one, so it keeps the
    #: field and this enum is reported as a reach gap rather than used to judge anything.
    struct_collision: str = ""


@dataclass
class RustIpc:
    commands: dict[str, Command] = field(default_factory=dict)
    registered: list[str] = field(default_factory=list)
    dtos: dict[str, Dto] = field(default_factory=dict)
    enums: dict[str, SerdeEnum] = field(default_factory=dict)
    handler_found: bool = False
    #: (file, bare name) -> declaration. `dtos`/`enums` are keyed by bare name and the last file
    #: parsed wins, which is fine until two files declare the same name — `Action` is a struct in
    #: `core/src/rules.rs` and an enum in `editor-shell/src/actions.rs`, both legitimate, both
    #: unambiguous in Rust because Rust resolves through module paths. These let a lookup prefer the
    #: declaration in the SAME FILE as the field that names it, which is what a bare `Action` in
    #: that file can only mean.
    dtos_local: dict[tuple[str, str], Dto] = field(default_factory=dict)
    enums_local: dict[tuple[str, str], SerdeEnum] = field(default_factory=dict)


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


def _snake_from_pascal(v: str) -> str:
    """serde's `SnakeCase.apply_to_variant`: an `_` before every uppercase but the first.

    `Blend1d` becomes `blend1d`, NOT `blend_1d` — a digit is not an uppercase letter, so serde does
    not break there. Getting that wrong in either direction is a false finding, and a false finding
    on a gate this quiet costs it the authority it needs.
    """
    out = []
    for i, ch in enumerate(v):
        if i > 0 and ch.isupper():
            out.append("_")
        out.append(ch.lower())
    return "".join(out)


def wire_case(name: str, rename_all: str | None) -> str:
    """serde's `RenameRule::apply_to_FIELD` — the input is a snake_case Rust field name.

    Field rules and variant rules are DIFFERENT functions in serde and must be different functions
    here. Under `rename_all = "lowercase"` a field is left ALONE (it is already lowercase); the
    variant `ReferencePose` becomes `referencepose`. One table serving both is one table that is
    wrong for one of them.
    """
    if rename_all in (None, "lowercase", "snake_case"):
        return name
    if rename_all in ("UPPERCASE", "SCREAMING_SNAKE_CASE"):
        return name.upper()
    if rename_all == "camelCase":
        return camel(name)
    if rename_all == "PascalCase":
        c = camel(name)
        return c[:1].upper() + c[1:]
    if rename_all == "kebab-case":
        return name.replace("_", "-")
    if rename_all == "SCREAMING-KEBAB-CASE":
        return name.upper().replace("_", "-")
    return name


def variant_case(name: str, rename_all: str | None) -> str:
    """serde's `RenameRule::apply_to_VARIANT` — the input is a PascalCase Rust variant name."""
    if rename_all in (None, "PascalCase"):
        return name
    if rename_all == "lowercase":
        return name.lower()
    if rename_all == "UPPERCASE":
        return name.upper()
    if rename_all == "camelCase":
        return name[:1].lower() + name[1:]
    if rename_all == "snake_case":
        return _snake_from_pascal(name)
    if rename_all == "SCREAMING_SNAKE_CASE":
        return _snake_from_pascal(name).upper()
    if rename_all == "kebab-case":
        return _snake_from_pascal(name).replace("_", "-")
    if rename_all == "SCREAMING-KEBAB-CASE":
        return _snake_from_pascal(name).upper().replace("_", "-")
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


def variant_rename(attrs: str) -> str | None:
    """A per-variant `#[serde(rename = "..")]`, which beats `rename_all` outright.

    Its own function so the self-test can replace it and prove that ignoring it produces a FALSE
    finding — the failure mode that matters most for a check this quiet. `Blend1d` is on the wire as
    `blend_1d` in this repository only because someone wrote this attribute; the mechanical
    snake_case of `Blend1d` is `blend1d`, and a reader that skipped the attribute would confidently
    report a drift that is not there.
    """
    m = _RENAME.search(attrs)
    return m.group(1) if m else None


def _parse_enums(src: str, rel: str, out: RustIpc) -> None:
    """`#[derive(Serialize)] enum X { .. }` resolved to the strings it can put on the wire.

    Three things decide the wire name of a variant and all three are read here: the enum's
    `rename_all` (through `variant_case`, which is NOT the field rule), a per-variant
    `#[serde(rename = "..")]` (which wins outright — `Blend1d` is on the wire as `blend_1d` in this
    repository only because someone wrote that attribute, and a reader that ignored it would report
    a drift that is not there), and `#[serde(skip)]`, which takes the variant off the wire entirely.

    An enum that is not a bare string on the wire is recorded with `string_like=False` and the
    reason, never dropped: dropping it would make "I cannot compare this" indistinguishable from
    "these agree", which is the one outcome this whole tool exists to prevent.
    """
    for m in _DERIVE.finditer(src):
        derives = m.group(1)
        if "Serialize" not in derives and "Deserialize" not in derives:
            continue
        tail = src[m.end() : m.end() + 800]
        em = re.search(r"\b(?:pub(?:\([^)]*\))?\s+)?enum\s+([A-Za-z_]\w*)(?:<[^>{]*>)?\s*\{", tail)
        if not em:
            continue
        attrs = tail[: em.start()]
        # Another item in between means this derive belongs to something else.
        if re.search(r"\b(struct|enum|fn|impl|trait)\b", attrs):
            continue
        ob = m.end() + em.end() - 1
        cb = _match(src, ob, "{", "}")
        if cb < 0:
            continue
        name = em.group(1)
        line = src.count("\n", 0, m.start()) + 1
        ra = _RENAME_ALL.search(attrs)
        rename_all = ra.group(1) if ra else None

        why_not = ""
        if re.search(r"#\[serde\([^\]]*\buntagged\b", attrs):
            why_not = "it is #[serde(untagged)], so a variant is its payload and has no name at all"
        elif re.search(r"#\[serde\([^\]]*\btag\s*=", attrs):
            why_not = "it is internally/adjacently tagged, so it is an OBJECT on the wire, not a string"

        variants: list[str] = []
        has_other = False
        for p in split_top(src[ob + 1 : cb]):
            p = p.strip()
            if not p:
                continue
            vattrs = " ".join(re.findall(r"#\[serde\(([^\]]*)\)\]", p))
            decl = re.sub(r"#\[[^\]]*\]", " ", p).strip()
            vm = re.match(r"([A-Za-z_]\w*)\s*(\(|\{)?", decl)
            if not vm:
                continue
            if re.search(r"(^|[\s,(])skip(?:_serializing)?(\s*[,)]|$)", vattrs):
                continue  # off the wire entirely
            if re.search(r"(^|[\s,(])other(\s*[,)]|$)", vattrs):
                has_other = True
            forced = variant_rename(vattrs)
            if vm.group(2) and not why_not:
                why_not = (
                    f"`{vm.group(1)}` carries data, so serde writes it externally tagged as "
                    f'`{{"{vm.group(1)}": ..}}` — an object, which a string union cannot describe'
                )
            variants.append(forced if forced else variant_case(vm.group(1), rename_all))

        prior = out.enums.get(name)
        if prior is not None and prior.variants != tuple(variants):
            # Same bare name, different variants, two files: `enums` is keyed by bare name, so the
            # second silently wins and a verdict derived from the wrong one is confident and wrong.
            why_not = why_not or (
                f"a different `{name}` is declared at {prior.file}:{prior.line} — the reader cannot "
                "tell which one a field means"
            )
        e = SerdeEnum(
            name,
            tuple(variants),
            line,
            rel,
            rename_all=rename_all,
            string_like=not why_not,
            why_not=why_not,
            has_other=has_other,
        )
        out.enums[name] = e
        out.enums_local[(rel, name)] = e


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
        dto = Dto(
            name, fields, line, rel, opaque=tuple(opaque), collides_with=collides, field_types=ftypes
        )
        out.dtos[name] = dto
        out.dtos_local[(rel, name)] = dto


_JSON_MACRO = re.compile(r"\bserde_json::json!\s*\(\s*\{")
#: The tail of a `.map(..).collect()` / `.collect::<Vec<_>>()` chain. A chain that does NOT collect
#: is an iterator, not an array, and the reply cannot contain one.
_COLLECT_TAIL = re.compile(r"\.\s*collect\s*(?:::\s*<[^;]*>\s*)?\(\s*\)\s*$")
_MAP_CALL = re.compile(r"\.\s*map\s*\(")

#: One entry of a `json!` object literal: its wire key, what kind of value it carries, and — when
#: that value is itself enumerable — the entries of the object behind it.
#: kind is "obj" (a nested `{..}` literal), "list" (a `.map(|x| json!({..})).collect()` of one), or
#: "opaque" (anything this reader cannot prove the shape of, which is most values and must stay
#: most values).
JsonEntry = tuple[str, str, "list | None"]


def _json_entries(inner: str) -> list[JsonEntry] | None:
    """The entries of one `json!({..})` object literal, recursively, or None if ANY defeats it.

    All-or-nothing on purpose. A partially enumerated object is the one outcome that turns this
    reader into a liar: `_read_findings` calls a field absent when it is not among the keys a
    struct carries, so an object missing three of its keys would report three FALSE findings about
    code that is correct. Returning None costs reach and keeps the walk stopping where it stops
    today, which is the honest half of the trade.
    """
    entries: list[JsonEntry] = []
    for entry in split_top(inner):
        km = re.match(r'^"([^"]*)"\s*:', entry.strip())
        if not km:
            return None  # a computed key, or a shape this reader does not understand
        entries.append((km.group(1), *_json_value_shape(entry.strip()[km.end() :])))
    return entries or None


def _json_value_shape(value: str) -> tuple[str, list[JsonEntry] | None]:
    """What one entry's VALUE is, as far as this reader can PROVE — never as far as it can guess.

    Two forms are enumerable and both are syntactic; there is no expression evaluation here and no
    following of a local binding to its initializer:

      * a nested object literal — `"working": { "current": .., "wired": true }`;
      * an array of them built the only way `json!` allows, since the macro cannot parse a method
        chain on an array literal: `xs.iter().map(|x| json!({..})).collect::<Vec<_>>()`.

    Everything else is opaque, and opaque means the walk stops exactly where it stopped before this
    function existed. The `.map()` form requires EXACTLY ONE `json!` in the expression: two means a
    conditional, a nested map, or a shape with more than one possible reading, and picking one of
    them would be the confident-wrong-answer class rather than the admitted-unknown one.
    """
    v = value.strip()
    if v.startswith("{"):
        cb = _match(v, 0, "{", "}")
        if cb == len(v) - 1:
            sub = _json_entries(v[1:cb])
            return ("obj", sub) if sub is not None else ("opaque", None)
        return ("opaque", None)
    macros = list(_JSON_MACRO.finditer(v))
    if len(macros) == 1 and _COLLECT_TAIL.search(v) and _MAP_CALL.search(v, 0, macros[0].start()):
        ob = v.index("{", macros[0].end() - 1)
        cb = _match(v, ob, "{", "}")
        if cb > 0:
            sub = _json_entries(v[ob + 1 : cb])
            return ("list", sub) if sub is not None else ("opaque", None)
    return ("opaque", None)


def _json_literal_entries(body: str) -> list[JsonEntry] | None:
    """The entries of the one `json!({..})` a command returns, or None if it is not that simple.

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
        return _json_entries(body[ob + 1 : cb])
    return None


def _register_json_dto(
    entries: list[JsonEntry], name: str, line: int, rel: str, out: RustIpc
) -> None:
    """Register `name` and every enumerable object below it, as DTOs the shape walk can follow.

    The synthetic names carry `@`, `.` and `[]`, none of which can appear in a Rust type name, so a
    nested shape can never be mistaken for — or collide with — a declared struct.
    """
    fields: list[tuple[str, bool]] = []
    ftypes: dict[str, str] = {}
    for key, kind, sub in entries:
        # A `json!` literal always emits its key. The VALUE may be `null`; the key is not absent,
        # which is the same reading the top-level keys have had since this resolver existed.
        fields.append((key, False))
        if sub is None:
            continue  # opaque: no field type, so the walk stops here exactly as it did before
        child = f"{name}.{key}" + ("[]" if kind == "list" else "")
        _register_json_dto(sub, child, line, rel, out)
        ftypes[key] = f"Vec<{child}>" if kind == "list" else child
    out.dtos[name] = Dto(name, fields, line, rel, origin="json!", field_types=ftypes)


def attach_json_returns(src: str, rel: str, out: RustIpc) -> None:
    """Resolve `-> serde_json::Value` commands to the literal they build, where that is unambiguous.

    Recursive since 2026-08-25 (ADR-142). It used to stop at the top-level keys, so a reply's every
    sub-object was a key that existed with nothing enumerable inside it, and every read below the
    first step was counted as `walk stopped early` rather than compared. That is how renaming
    `asDirected` inside `camera_probe`'s `shotPlacements` elements left this gate at `0 blocking`
    while the spec that reads it published `reaimed: 15 of 15` — a false number about the film, from
    a green run, because `!undefined` is `true`.
    """
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
        entries = _json_literal_entries(src[ob + 1 : cb])
        if entries is None:
            continue
        synth = f"json!@{cmd.name}"
        _register_json_dto(entries, synth, cmd.line, rel, out)
        cmd.ret = synth


def parse(sources: list[tuple[str, str]]) -> RustIpc:
    """`sources` is [(relative path, text)]; later files may add DTOs the earlier ones return."""
    out = RustIpc()
    cleaned = [(rel, strip_line_comments(text)) for rel, text in sources]
    for rel, src in cleaned:
        _parse_commands(src, rel, out)
        _parse_handler(src, out)
        _parse_structs(src, rel, out)
        _parse_enums(src, rel, out)
    for rel, src in cleaned:
        attach_json_returns(src, rel, out)
    # A bare name that is BOTH a struct and an enum cannot be resolved by name, and resolving it
    # anyway is worse than not resolving it: `Action` is a struct in core/src/rules.rs AND an enum
    # in editor-shell/src/actions.rs, so a field typed `Action` would be compared against whichever
    # kind the reader happened to try first — an object against a string union, confidently. This
    # runs after every file because the two declarations need not be in the same one.
    # A bare name that is BOTH a struct and an enum is only ambiguous to a reader that keys by bare
    # name; it is never ambiguous in Rust. `Action` is a struct in core/src/rules.rs and an enum in
    # editor-shell/src/actions.rs, and a bare `Action` written in either file can only mean that
    # file's own declaration. So the collision is recorded — and resolution prefers the SAME FILE
    # (see `local_dto` / `local_enum`) rather than whichever kind the reader happened to try first.
    for name, e in out.enums.items():
        d = out.dtos.get(name)
        if d is not None:
            e.struct_collision = f"{d.file}:{d.line}"
    return out


def local_dto(name: str, out: RustIpc, prefer_file: str | None) -> Dto | None:
    """The struct `name` means in `prefer_file`, falling back to the bare-name table.

    A declaration in the file that names it wins outright: Rust would reject a bare reference to a
    same-named type from elsewhere without a `use` that shadowed it, so there is no second reading.
    """
    if prefer_file is not None:
        d = out.dtos_local.get((prefer_file, name))
        if d is not None:
            return d
        if (prefer_file, name) in out.enums_local:
            return None  # locally an ENUM; the global struct of that name is a different type
    return out.dtos.get(name)


def local_enum(name: str, out: RustIpc, prefer_file: str | None) -> SerdeEnum | None:
    """The enum `name` means in `prefer_file`, falling back to the bare-name table."""
    if prefer_file is not None:
        e = out.enums_local.get((prefer_file, name))
        if e is not None:
            return e
        if (prefer_file, name) in out.dtos_local:
            return None  # locally a STRUCT; the global enum of that name is a different type
    return out.enums.get(name)
