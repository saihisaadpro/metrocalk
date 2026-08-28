//! **The Material picker** (ADR-164) — the Inspector section that answers "what is this made of?" and
//! lets an author change the answer in one free, undoable click.
//!
//! Implement this feature in accordance with the Engine UI/UX Architecture Constitution.
//!
//! WHAT WAS HERE, AND WHY IT WAS A DEFECT RATHER THAN A STYLE. The Inspector's `Material` section
//! rendered `AiEditPanel`, whose palette was six text buttons, and every one of them called
//! `client.aiEdit` → the metered `ai_edit` command → `wallet.charge(&Action::Edit)`. The op that command
//! ultimately performs is `SetField { component: "MeshRenderer", field: "material" }` — the identical op
//! the Inspector two inches above emits for free on every other field, through the same transactional,
//! undoable pipeline. So the engine charged two tokens to write a string it writes for nothing
//! elsewhere, and an author who was offline or out of tokens could not change a material at all. The
//! deterministic core is the product and the AI is a guest; a guest had been left holding the only key.
//!
//! WHAT REPLACES IT. The six finishes the renderer knows, each drawn as the surface it actually is
//! (`theme/materials.tsx`), the current one carrying the selection, and a `Default` swatch that hands
//! the object back its own appearance. One `client.setField` per pick — free, instant, undoable with
//! Ctrl-Z like any other edit. The AI suggestion stays, below the palette and priced, because it is
//! still the only thing here that can invent a finish nobody has named.
//!
//! THE REFUSAL IS STATED BEFORE THE CLICK, NOT AFTER IT. `SetField` needs the component to exist, so an
//! object with no `MeshRenderer` cannot take a finish. That used to be discovered by pressing a live
//! button and reading an error; the swatches are now disabled and say why.

import { AiEditPanel } from "./AiEditPanel";
import { useSelectedId, useDisplayedEntity } from "../store/projection";
import { setStatus } from "../store/ui";
import { pushToast } from "../store/toasts";
import { Callout } from "../theme/fields";
import { SwatchGrid, SwatchTile } from "../theme/assets";
import { Icon } from "../theme/icons";
import {
  MATERIAL_DEFAULT,
  MATERIAL_PRESETS,
  MaterialSphere,
  materialPresetFor,
  type MaterialPreset,
} from "../theme/materials";
import type { EditorClient } from "../transport/session";

/** The cleared state, drawn as a swatch so it sits in the same row as the finishes it competes with.
 *  A neutral mark rather than a sphere: it is the ABSENCE of an override, and drawing it as a grey ball
 *  would claim the object is grey, which is exactly what it is not. */
function DefaultPreview() {
  return (
    <span className="mtk-swatch__none" aria-hidden="true">
      <Icon name="revert" size="lg" />
    </span>
  );
}

export function MaterialPanel({ client }: { client: EditorClient }) {
  const selectedId = useSelectedId();
  const entity = useDisplayedEntity(selectedId ?? "");

  if (!selectedId) return null;

  // `SetField` commits against a component that EXISTS, so the presence of `MeshRenderer` — not of the
  // material field inside it — is what decides whether this object can take a finish at all.
  const renderer = entity?.components.MeshRenderer;
  const shadeable = renderer !== undefined;
  const stored = renderer?.material;
  const current = materialPresetFor(stored);
  // A value the renderer would ignore, and that is not the cleared word either: an imported material
  // handle, or something typed into the asset field by hand. The grid cannot show it, so the panel says
  // it in words rather than silently drawing nothing as selected.
  const foreign =
    typeof stored === "string" && stored !== "" && stored !== MATERIAL_DEFAULT && current === null
      ? stored
      : null;
  // "Nothing is overriding this object's own appearance" — but only where there is an appearance to
  // override. On an object that cannot take a finish at all, a lit `Default` tile reads as a current
  // setting inside a panel that has just said none is possible, which is two statements disagreeing
  // in the same 234px column. Caught by reading the `material-no-mesh` capture.
  const cleared = shadeable && current === null && foreign === null;
  const refusal = shadeable ? undefined : "This object has no mesh to shade — add a MeshRenderer first.";

  function pick(value: string, label: string) {
    if (!selectedId || !shadeable) return;
    client.setField(selectedId, "MeshRenderer", "material", value);
    // Feedback at the gesture, and a stable token the E2E can key on that names the finish rather than
    // quoting prose. The swatch's own selection is the durable confirmation; the toast is the receipt.
    pushToast(`${label} · applied`, "success");
    setStatus(`material ${value}`);
  }

  return (
    <div id="materialPicker" data-testid="materialPicker" className="mtk-material">
      {refusal != null && (
        <Callout tone="neutral" data-testid="material-unavailable">
          {refusal}
        </Callout>
      )}
      {foreign != null && (
        <Callout tone="info" data-testid="material-foreign" title="Imported finish">
          This object uses <span className="mtk-material__handle">{foreign}</span> from its source file.
          Picking a finish below overrides it; <strong>Default</strong> gives it back.
        </Callout>
      )}
      <SwatchGrid label="Surface finish">
        <SwatchTile
          data-testid="material-default"
          label="Default"
          preview={<DefaultPreview />}
          actionLabel="Use this object's own appearance"
          title="Clear the finish — the object goes back to the material it was imported or created with"
          selected={cleared}
          disabled={!shadeable}
          disabledReason={refusal}
          onSelect={() => pick(MATERIAL_DEFAULT, "Default appearance")}
        />
        {MATERIAL_PRESETS.map((preset: MaterialPreset) => (
          <SwatchTile
            key={preset.id}
            data-testid={`material-${preset.id}`}
            label={preset.label}
            preview={<MaterialSphere preset={preset} />}
            actionLabel={`Give this object a ${preset.label.toLowerCase()} finish`}
            title={preset.hint}
            selected={current?.id === preset.id}
            disabled={!shadeable}
            disabledReason={refusal}
            onSelect={() => pick(preset.id, `${preset.label} finish`)}
          />
        ))}
      </SwatchGrid>
      <p className="mtk-material__note">
        A finish is an ordinary edit — free, instant, and undone with Ctrl&nbsp;+&nbsp;Z.
      </p>
      <AiEditPanel client={client} />
    </div>
  );
}
