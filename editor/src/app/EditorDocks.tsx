//! The editor's stable information architecture.
//!
//! **One rule, and it is the whole design:** the LEFT column is the sub-engine you are working in (chosen
//! from the Engines rail — see `EngineRail`), and the RIGHT column is the thing you have selected. Nothing
//! else moves. Before this, terrain was a right-hand tab, animation was at the bottom and physics was on the
//! right, which made the docks *positions* rather than categories — an author could not predict where
//! anything lived.
//!
//! The left dock therefore has no tab strip of its own: the rail is its tab strip, and duplicating it here
//! would re-create the ambiguity this layout exists to remove.

import { lazy, useRef, useState } from "react";
import { AuthoringToolbar } from "../panels/AuthoringToolbar";
import { Hierarchy } from "../panels/Hierarchy";
import { Requirers } from "../panels/Requirers";
import { Reveal } from "../panels/Reveal";
import { Icon } from "../theme/icons";
import { Button } from "../theme/primitives";
import { Popover, PopoverSurface } from "../theme/Popover";
import { DisclosureSection, DockTabs, EmptyPanelState, WorkspacePanel } from "../theme/workspace";
import { color } from "../theme/tokens";
import { useEntityOrder, useSelectedId } from "../store/projection";
import type { EditorClient } from "../transport/session";
import { LazyWorkspace } from "./LazyWorkspace";

// Build and Terrain are explicit rail destinations, not startup chrome. Keeping them behind the same
// shared workspace boundary as the other engines protects the stage's initial bundle while preserving
// a named, polite loading state when an author actually opens either workspace.
const AssetBrowser = lazy(() => import("../panels/AssetBrowser").then((module) => ({ default: module.AssetBrowser })));
const ShapeStudio = lazy(() => import("../panels/ShapeStudio").then((module) => ({ default: module.ShapeStudio })));
const TerrainPanel = lazy(() => import("../panels/TerrainPanel").then((module) => ({ default: module.TerrainPanel })));
const Diagnostics = lazy(() => import("../panels/Diagnostics").then((module) => ({ default: module.Diagnostics })));
const MaterialPanel = lazy(() => import("../panels/MaterialPanel").then((module) => ({ default: module.MaterialPanel })));
const JointPanel = lazy(() => import("../panels/JointPanel").then((module) => ({ default: module.JointPanel })));
const PhysicsPanel = lazy(() => import("../panels/PhysicsPanel").then((module) => ({ default: module.PhysicsPanel })));
const MatchPanel = lazy(() => import("../panels/MatchPanel").then((module) => ({ default: module.MatchPanel })));
const TransformPanel = lazy(() => import("../panels/TransformPanel").then((module) => ({ default: module.TransformPanel })));
const Inspector = lazy(() => import("../inspector/Inspector").then((module) => ({ default: module.Inspector })));
const BehaviourSection = lazy(() => import("../panels/BehaviourSection").then((module) => ({ default: module.BehaviourSection })));
const BindingGraph = lazy(() => import("../graph/BindingGraph").then((module) => ({ default: module.BindingGraph })));

/// The sub-engines whose workspace is a side panel (a stack of controls). The wide ones — model, import,
/// animate, logic — open in the bottom dock instead, because a timeline needs width.
export type LeftWorkspace = "scene" | "build" | "terrain" | "physics" | "gameplay";
/// The Inspector answers ONE question — "what is selected?" — so it holds only selection-scoped surfaces.
/// Terrain, physics and match used to live here and are systems, not selections; they moved to the rail.
export type InspectorWorkspace = "properties" | "relations";

export interface LeftDockProps {
  client: EditorClient;
  active: LeftWorkspace;
  onContextMenu: (id: string, x: number, y: number) => void;
  onStartPipe: () => void;
  onImport: () => void;
  onCollapse?: () => void;
  onPin?: () => void;
  /** Open the Cutscene timeline in the Animate dock (the Gameplay workspace's Cinematics block). */
  onOpenCutscene?: () => void;
}

function DockChromeAction({ onCollapse, onPin, label }: { onCollapse?: () => void; onPin?: () => void; label: string }) {
  if (!onCollapse && !onPin) return null;
  const pinning = !!onPin;
  return (
    <Button
      variant="ghost"
      compact
      icon
      aria-label={pinning ? `Pin ${label} dock` : `Collapse ${label} dock`}
      title={pinning ? "Pin this panel open" : "Collapse to a floating tool rail"}
      onClick={pinning ? onPin : onCollapse}
    >
      <Icon name={pinning ? "pin" : "chevron-left"} size="md" />
    </Button>
  );
}

function NeedsAttentionPopup() {
  const [open, setOpen] = useState(false);
  const triggerRef = useRef<HTMLButtonElement>(null);
  return (
    <>
      <Button
        ref={triggerRef}
        data-testid="scene-attention"
        variant="ghost"
        compact
        icon
        aria-label="Objects needing attention"
        aria-haspopup="dialog"
        aria-expanded={open}
        title="Objects waiting for a compatible binding"
        onClick={() => setOpen((current) => !current)}
      >
        <Icon name="problems" size="md" />
      </Button>
      <Popover
        open={open}
        anchor={triggerRef}
        returnFocus={triggerRef}
        placement="bottom-end"
        role="dialog"
        ariaLabel="Objects needing attention"
        onClose={() => setOpen(false)}
      >
        <PopoverSurface className="mtk-compact-popup">
          <div className="mtk-compact-popup__header">
            <div>
              <strong>Needs attention</strong>
              <span>Objects waiting for a compatible binding</span>
            </div>
            <Button variant="ghost" compact icon aria-label="Close objects needing attention" onClick={() => setOpen(false)}>×</Button>
          </div>
          <div className="mtk-compact-popup__body"><Requirers /></div>
        </PopoverSurface>
      </Popover>
    </>
  );
}

/** Title and subtitle for the side engine currently open, so the panel always says where you are. */
function sideEngineChrome(active: LeftWorkspace, entities: number, selected: string | null) {
  switch (active) {
    case "scene":
      return { title: "Scene", subtitle: `${entities.toLocaleString("en-GB")} objects` };
    case "build":
      return { title: "Build", subtitle: "Shapes, drawing and the asset library" };
    case "terrain":
      return { title: "Terrain", subtitle: "Landscape, water and vegetation" };
    case "physics":
      return { title: "Physics", subtitle: selected ? "Simulation and collision" : "Whole-scene simulation" };
    case "gameplay":
      return { title: "Gameplay", subtitle: "Author and run this scene as a match" };
  }
}

export function LeftDock({ client, active, onContextMenu, onStartPipe, onImport, onCollapse, onPin, onOpenCutscene }: LeftDockProps) {
  const entities = useEntityOrder();
  const selected = useSelectedId();
  const chrome = sideEngineChrome(active, entities.length, selected);
  return (
    <WorkspacePanel
      data-testid="left-dock"
      title={chrome.title}
      subtitle={chrome.subtitle}
      actions={active === "scene" || onCollapse || onPin ? (
        <>
          {active === "scene" && <NeedsAttentionPopup />}
          {(onCollapse || onPin) && <DockChromeAction label="left" onCollapse={onCollapse} onPin={onPin} />}
        </>
      ) : undefined}
      actionsLabel="Workspace actions"
      scroll={false}
    >
      {/* Each panel is a `tabpanel` of the RAIL's `tablist`. That is what makes the rail and this column one
          control rather than two that happen to agree with each other. */}
      <div
        id="engine-panel-scene"
        role="tabpanel"
        aria-labelledby="engine-tab-scene"
        hidden={active !== "scene"}
        className="mtk-dock-panel"
      >
        <AuthoringToolbar client={client} />
        <div className="mtk-dock-panel__fill"><Hierarchy client={client} onContextMenu={onContextMenu} /></div>
      </div>
      <div
        id="engine-panel-build"
        role="tabpanel"
        aria-labelledby="engine-tab-build"
        hidden={active !== "build"}
        className="mtk-dock-panel mtk-scroll"
      >
        {active === "build" && (
          <LazyWorkspace label="Build workspace">
            <ShapeStudio client={client} />
            <div className="mtk-dock-section-heading">Other tools</div>
            <div className="mtk-quick-create" role="group" aria-label="Quick creation tools">
              <Button data-testid="create-pipe" variant="secondary" onClick={onStartPipe} title="Draw a production pipe asset directly in the viewport">
                <Icon name="pipe" size="md" /> Draw pipe
              </Button>
              <Button data-testid="create-import" variant="secondary" onClick={onImport} title="Choose a supported local asset file">
                <Icon name="import" size="md" /> Import
              </Button>
            </div>
            {/* WHERE `Describe` WENT. It was a collapsed `DisclosureSection` here, summarised "Optional
                assisted creation", below Shape Studio and Other tools — three clicks and a scroll from
                the stage for north star #2, under a label telling you not to bother. It is the stage
                composer now (`.mtk-stage-footer`), always present, and this column keeps the two things
                a Build workspace is actually for: making a shape, and finding one. */}
            <div className="mtk-dock-section-heading">Asset library</div>
            <AssetBrowser client={client} />
          </LazyWorkspace>
        )}
      </div>
      <div
        id="engine-panel-terrain"
        role="tabpanel"
        aria-labelledby="engine-tab-terrain"
        hidden={active !== "terrain"}
        className="mtk-dock-panel mtk-scroll"
      >
        {active === "terrain" && <LazyWorkspace label="Terrain workspace"><TerrainPanel client={client} /></LazyWorkspace>}
      </div>
      <div
        id="engine-panel-physics"
        role="tabpanel"
        aria-labelledby="engine-tab-physics"
        hidden={active !== "physics"}
        className="mtk-dock-panel mtk-scroll"
      >
        {active === "physics" && <LazyWorkspace label="Physics workspace"><PhysicsPanel client={client} /></LazyWorkspace>}
      </div>
      <div
        id="engine-panel-gameplay"
        role="tabpanel"
        aria-labelledby="engine-tab-gameplay"
        hidden={active !== "gameplay"}
        className="mtk-dock-panel mtk-scroll"
      >
        {active === "gameplay" && <LazyWorkspace label="Gameplay workspace"><MatchPanel client={client} onOpenTimeline={onOpenCutscene} /></LazyWorkspace>}
      </div>
    </WorkspacePanel>
  );
}

export interface InspectorDockProps {
  client: EditorClient;
  active: InspectorWorkspace;
  onChange: (workspace: InspectorWorkspace) => void;
  onCollapse?: () => void;
  onPin?: () => void;
  /** Jump the left rail to a deeper workspace (the Behaviour block's "Animate/Gameplay" links). */
  onJumpTo?: (engine: "animate" | "gameplay") => void;
}

export function InspectorDock({ client, active, onChange, onCollapse, onPin, onJumpTo }: InspectorDockProps) {
  const selected = useSelectedId();
  const tabs = [
    { id: "properties", label: "Properties", icon: "properties", tooltip: "Edit the selected object's components and material" },
    { id: "relations", label: "Relations", icon: "relations", tooltip: "Inspect compatible bindings and graph relationships" },
  ] as const;
  return (
    <WorkspacePanel
      data-testid="inspector-dock"
      title="Inspector"
      subtitle={selected ? "Selection context" : "Nothing selected"}
      actions={onCollapse || onPin ? <DockChromeAction label="Inspector" onCollapse={onCollapse} onPin={onPin} /> : undefined}
      actionsLabel="Inspector dock layout"
      scroll={false}
      tabs={<DockTabs id="inspector-workspaces" ariaLabel="Inspector workspaces" tabs={tabs.map((tab) => ({ ...tab, icon: <Icon name={tab.icon} size="md" /> }))} activeId={active} onChange={(id) => onChange(id as InspectorWorkspace)} />}
    >
      <div
        id="inspector-workspaces-properties-panel"
        role="tabpanel"
        aria-labelledby="inspector-workspaces-properties-tab"
        hidden={active !== "properties"}
        className="mtk-dock-panel mtk-scroll"
      >
        {active === "properties" && (
          <LazyWorkspace label="Properties inspector">
            <Inspector client={client} />
            {selected ? (
              <>
                <BehaviourSection client={client} onJumpTo={onJumpTo} />
                <DisclosureSection title="Diagnostics" summary="Selection health and quick fixes" defaultOpen storageKey="inspect-diagnostics">
                  <Diagnostics client={client} />
                </DisclosureSection>
                {/* OPEN BY DEFAULT (ADR-164). "What is this made of" is answered by looking, and the
                    answer is a grid of shaded spheres that takes one glance — the reference sheet's
                    Materials block is the first thing in its right column for the same reason. It was
                    collapsed while it held six text buttons that spent tokens, which was the correct
                    default for what it was and the wrong one for what it is. */}
                <DisclosureSection title="Material" summary="Surface finish" defaultOpen storageKey="inspect-material">
                  <MaterialPanel client={client} />
                </DisclosureSection>
                <DisclosureSection title="Object actions" summary="Reuse, hierarchy and placement" defaultOpen={false} storageKey="inspect-object-actions">
                  <TransformPanel client={client} />
                </DisclosureSection>
                <DisclosureSection title="Mechanism" summary="Joint and keyframe authoring" defaultOpen={false} storageKey="inspect-mechanism">
                  <JointPanel client={client} />
                </DisclosureSection>
              </>
            ) : null}
          </LazyWorkspace>
        )}
      </div>
      <div
        id="inspector-workspaces-relations-panel"
        role="tabpanel"
        aria-labelledby="inspector-workspaces-relations-tab"
        hidden={active !== "relations"}
        className="mtk-dock-panel"
      >
        {active === "relations" && selected ? (
          <>
            <div className="mtk-dock-panel__scroll mtk-scroll"><Reveal client={client} /></div>
            <div className="mtk-dock-panel__fill" style={{ borderTop: `1px solid ${color.border.subtle}`, minHeight: 220 }}><LazyWorkspace label="Relationship graph"><BindingGraph /></LazyWorkspace></div>
          </>
        ) : (
          <EmptyPanelState title="Select an object to inspect its relationships" description="Compatible bindings and the local relationship graph will appear here." icon={<Icon name="relations" size="xl" />} />
        )}
      </div>
    </WorkspacePanel>
  );
}
