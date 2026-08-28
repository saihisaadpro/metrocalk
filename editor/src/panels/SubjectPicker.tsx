//! **What a shot frames** — the picker for a shot's subject.
//!
//! WHAT WAS MISSING, AND WHERE IT ALREADY EXISTED. A `ShotRecipe` has carried its own `subject` since
//! cutscenes shipped: the runtime resolves it as the union of every rendered instance in that
//! object's HIERARCHY SUBTREE, so "film the whole assembly" has always been a thing the solver could
//! do, `cinema_add_shot` has always taken a subject, and the shot list has always captioned each line
//! with the name of whatever that shot films. The editor sent no subject and offered no way to change
//! one, so in practice every shot filmed the object its cutscene hung on — and the single most
//! ordinary cinematic sequence there is, *hold on the whole line, then cut in to this one machine*,
//! could not be authored at all.
//!
//! WHY A PICKER AND NOT A DROPDOWN. The benchmark scene imports as 15,711 parts. A `<select>` of the
//! scene is not a control, and a bare text field asking for an entity key is not one either. The
//! engine answers with a RANKED list built from the scene's own hierarchy — this object, what it is
//! part of, what it is made of, what stands beside it — so the four subjects a person actually wants
//! are the first four rows, and the search box is for the fifth.
//!
//! AND THE FIRST ROW IS NOT A ROW. A list is how you reach something you can NAME. The most common
//! thing an author wants to film is the one they are looking at — so the picker opens on "Click it in
//! the viewport", which hands the choice to the stage (`store/subjectAim`), where the object under
//! the cursor is named with its part count and its whole ancestor chain is one click each. The list
//! below it is for everything that is off screen, behind something, or easier to name than to find.
//!
//! THE NUMBER BESIDE EACH ROW IS THE POINT. `parts` is how many DRAWN instances sit under a
//! candidate, counted off the same published render list the shot solver fits its camera to. It is
//! what tells a 378-part assembly apart from the one bracket that shares most of its name — and a row
//! reading "nothing drawn" is the engine saying, before the choice is made, that this subject would
//! be framed at its own origin inside a metre-ish fallback box. That failure is invisible from the
//! outside: the camera goes somewhere plausible and points at nothing.

import { useCallback, useEffect, useRef, useState } from "react";
import { Popover, PopoverSurface } from "../theme/Popover";
import { Button, SearchField } from "../theme/primitives";
import { PopupMenuGroup, PopupMenuItem } from "../theme/workspace";
import { Icon } from "../theme/icons";
import { color, fontSize, space } from "../theme/tokens";
import type { SubjectCandidate, SubjectCatalog } from "../transport/protocol";
import type { EditorClient } from "../transport/session";

/** How long a keystroke waits before it becomes a scene-wide scan. The engine's search walks every
 *  entity in the document, so typing "weld" unthrottled is four full scans of a 15,711-part import
 *  for three answers nobody read. */
const SEARCH_DEBOUNCE_MS = 140;

const EMPTY: SubjectCatalog = {
  owner: "",
  ownerName: "",
  current: null,
  candidates: [],
  query: "",
  matches: 0,
  truncated: false,
};

/** `2 parts` / `1 part` / the honest absence. Plural rules stated once, here. */
function partsLabel(parts: number): string {
  if (parts === 0) return "nothing drawn";
  return `${parts} part${parts === 1 ? "" : "s"}`;
}

/** The candidates in the order the engine ranked them, split into the runs it headed. Grouping is a
 *  READ of `group`, never a re-sort: the engine decided the order and the headings are its words, so
 *  a heading here can never name a rank the engine did not produce. */
function groups(candidates: SubjectCandidate[]): { label: string; rows: SubjectCandidate[] }[] {
  const out: { label: string; rows: SubjectCandidate[] }[] = [];
  for (const row of candidates) {
    const last = out[out.length - 1];
    if (last && last.label === row.group) last.rows.push(row);
    else out.push({ label: row.group, rows: [row] });
  }
  return out;
}

export interface SubjectPickerProps {
  client: EditorClient;
  /** The object the cutscene hangs on — the picker asks the engine about the scene around it. */
  owner: string;
  /** The shot being aimed, so its current subject comes back ticked. `null` while no shot is open. */
  shotIndex: number | null;
  /** What the shot frames right now. */
  value: string;
  /** That object's display name, from the shot row — so the closed control reads without a fetch. */
  valueName: string;
  disabled?: boolean;
  /** Why the control is refusing, in the user's words — never a bare dark button. */
  disabledReason?: string;
  /** Ties the trigger to its `Field` label. */
  id?: string;
  onPick: (subject: string) => void;
  /** Hand the choice to the stage: close the picker, and let the next click on the viewport aim the
   *  shot. Optional so this control can be rendered without the stage behind it. */
  onAimInViewport?: () => void;
}

/**
 * A button that reads what the shot frames, and opens the scene's own hierarchy to change it.
 */
export function SubjectPicker({
  client,
  owner,
  shotIndex,
  value,
  valueName,
  disabled = false,
  disabledReason,
  id,
  onPick,
  onAimInViewport,
}: SubjectPickerProps) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [catalog, setCatalog] = useState<SubjectCatalog>(EMPTY);
  const [loading, setLoading] = useState(false);
  const anchor = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const search = useRef<HTMLInputElement>(null);

  const close = useCallback(() => {
    setOpen(false);
    // The next opening starts from the ranked list. A picker that remembered the last search would
    // reopen showing three rows out of a fifteen-thousand-part scene with no sign why.
    setQuery("");
  }, []);

  useEffect(() => {
    if (!open) return;
    let live = true;
    const run = () => {
      setLoading(true);
      client
        .cinemaSubjectCatalog(owner, shotIndex, query)
        .then((reply) => {
          if (live) setCatalog(reply);
        })
        .catch(() => {
          // A failed read leaves the last list rather than blanking the panel mid-decision.
        })
        .finally(() => {
          if (live) setLoading(false);
        });
    };
    // The FIRST list is not debounced: opening the picker and waiting 140ms for four rows the engine
    // already knows is a control that feels broken. Only typing pays the delay.
    if (!query) {
      run();
      return () => {
        live = false;
      };
    }
    const timer = setTimeout(run, SEARCH_DEBOUNCE_MS);
    return () => {
      live = false;
      clearTimeout(timer);
    };
  }, [client, open, owner, shotIndex, query]);

  // Focus lands in the search box, not on the first row: with the ranked list already showing what a
  // person most often wants, the keyboard's job here is to reach the thing that is NOT on it.
  useEffect(() => {
    if (open) search.current?.focus();
  }, [open]);

  const rows = catalog.candidates;
  const searching = catalog.query.length > 0;

  return (
    // FULL WIDTH, like every other control on this row. A flex box sized to its content made the
    // Frames control 153px in a 318px grid column beside three full-width selects — one short
    // control in a row of four, which reads as a different KIND of control rather than as the same
    // decision. Measured on the shots capture, which is the only place four of them are side by side.
    <div ref={anchor} style={{ display: "flex", minWidth: 0, width: "100%" }}>
      <Button
        ref={trigger}
        id={id}
        data-testid="cutscene-subject"
        variant="secondary"
        compact
        disabled={disabled}
        disabledReason={disabledReason}
        aria-haspopup="menu"
        aria-expanded={open}
        title={
          disabled
            ? disabledReason
            : `This shot frames ${valueName}. Click to point it at something else — the assembly it belongs to, one of its parts, or anything in the scene.`
        }
        style={{ minWidth: 0, justifyContent: "space-between", width: "100%" }}
        onClick={() => setOpen((o) => !o)}
      >
        <span
          style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}
          data-testid="cutscene-subject-name"
        >
          {valueName || value}
        </span>
        <Icon name="chevron-down" size="sm" />
      </Button>

      <Popover
        open={open}
        anchor={anchor}
        returnFocus={trigger}
        id="cutsceneSubjectPicker"
        ariaLabel="What this shot frames"
        onClose={close}
      >
        <PopoverSurface
          data-testid="cutscene-subject-picker"
          className="mtk-popup-menu"
          style={{ minWidth: 320, maxWidth: 460 }}
        >
          {/* POINTING BEFORE NAMING. The gesture that needs no vocabulary comes first, and it is
              separated from the search rather than sitting in it: they answer different questions —
              "that one, there" and "the one called X" — and a control that reads as a search result
              would be a search result nobody typed. */}
          {onAimInViewport && (
            <div style={{ borderBottom: `1px solid ${color.border.subtle}` }}>
              <PopupMenuItem
                data-testid="cutscene-subject-aim"
                label="Click it in the viewport"
                description="Point at the object on the stage. What you hover is named before you click, with what it is part of one click away."
                leading={<Icon name="cursor" size="sm" />}
                onSelect={() => onAimInViewport()}
                onRequestClose={close}
              />
            </div>
          )}

          <div style={{ padding: space.xs }}>
            <SearchField
              ref={search}
              data-testid="cutscene-subject-search"
              aria-label="Search the scene for something to frame"
              placeholder="Search the scene…"
              value={query}
              style={{ width: "100%" }}
              onChange={(event) => setQuery(event.currentTarget.value)}
            />
          </div>

          <div
            data-testid="cutscene-subject-list"
            style={{ maxHeight: 320, overflowY: "auto", overscrollBehavior: "contain" }}
          >
            {rows.length === 0 ? (
              <div
                data-testid="cutscene-subject-empty"
                style={{ padding: `${space.xs}px ${space.md}px`, color: color.text.muted, fontSize: fontSize.body }}
              >
                {loading
                  ? "Looking…"
                  : searching
                    ? `Nothing in the scene is called “${catalog.query}”.`
                    : "This scene has nothing to frame yet."}
              </div>
            ) : (
              groups(rows).map((group) => (
                <PopupMenuGroup key={group.label} label={group.label}>
                  {group.rows.map((row) => (
                    <PopupMenuItem
                      key={row.id}
                      className="cutscene-subject-option"
                      data-testid={`cutscene-subject-option-${row.id}`}
                      data-parts={row.parts}
                      aria-checked={row.current}
                      role="menuitemradio"
                      label={row.name}
                      // The count is the decision. "378 parts" and "1 part" are how an assembly and
                      // the bracket inside it tell themselves apart when their names do not.
                      meta={partsLabel(row.parts)}
                      description={
                        row.framable
                          ? undefined
                          : "Nothing under this is drawn — a shot of it would be composed on its origin."
                      }
                      leading={row.current ? <Icon name="check" size="sm" /> : undefined}
                      title={
                        row.framable
                          ? `Frame ${row.name} — ${partsLabel(row.parts)} under it`
                          : `${row.name} has no drawn geometry under it, so the camera would be fitted to its origin rather than to anything you can see.`
                      }
                      onSelect={() => onPick(row.id)}
                      onRequestClose={close}
                    />
                  ))}
                </PopupMenuGroup>
              ))
            )}
          </div>

          {/* NEVER IMPLY COMPLETENESS. Both groups and searches are bounded by the engine, and a list
              that silently stopped at twelve of four hundred siblings reads as "there are twelve". */}
          {catalog.truncated && (
            <div
              data-testid="cutscene-subject-truncated"
              style={{
                padding: `${space.xxs}px ${space.md}px ${space.xs}px`,
                borderTop: `1px solid ${color.border.subtle}`,
                color: color.text.muted,
                fontSize: fontSize.meta,
              }}
            >
              {searching
                ? `Showing ${rows.length} of ${catalog.matches} matches — narrow the search to see the rest.`
                : "More in the scene than fits here — search for it by name."}
            </div>
          )}
        </PopoverSurface>
      </Popover>
    </div>
  );
}

export default SubjectPicker;
