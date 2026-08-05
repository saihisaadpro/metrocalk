//! The editor's stable information architecture. Scene navigation and creation live on the left;
//! selection-context properties, relationships, and physics live on the right. Feature panels retain
//! their own state while tabs use predictable ARIA semantics and keyboard navigation.

import { lazy, useRef, useState } from "react";
import { AssetBrowser } from "../panels/AssetBrowser";
import { AuthoringToolbar } from "../panels/AuthoringToolbar";
import { DescribeBar } from "../panels/DescribeBar";
import { Hierarchy } from "../panels/Hierarchy";
import { Requirers } from "../panels/Requirers";
import { Reveal } from "../panels/Reveal";
import { Button } from "../theme/primitives";
import { Popover, PopoverSurface } from "../theme/Popover";
import { DisclosureSection, DockTabs, EmptyPanelState, WorkspacePanel } from "../theme/workspace";
import { color } from "../theme/tokens";
import { useEntityOrder, useSelectedId } from "../store/projection";
import type { EditorClient } from "../transport/session";
import { LazyWorkspace } from "./LazyWorkspace";

const Diagnostics = lazy(() => import("../panels/Diagnostics").then((module) => ({ default: module.Diagnostics })));
const AiEditPanel = lazy(() => import("../panels/AiEditPanel").then((module) => ({ default: module.AiEditPanel })));
const JointPanel = lazy(() => import("../panels/JointPanel").then((module) => ({ default: module.JointPanel })));
const PhysicsPanel = lazy(() => import("../panels/PhysicsPanel").then((module) => ({ default: module.PhysicsPanel })));
const MatchPanel = lazy(() => import("../panels/MatchPanel").then((module) => ({ default: module.MatchPanel })));
const TransformPanel = lazy(() => import("../panels/TransformPanel").then((module) => ({ default: module.TransformPanel })));
const Inspector = lazy(() => import("../inspector/Inspector").then((module) => ({ default: module.Inspector })));
const BindingGraph = lazy(() => import("../graph/BindingGraph").then((module) => ({ default: module.BindingGraph })));

export type LeftWorkspace = "scene" | "create";
export type InspectorWorkspace = "properties" | "relations" | "physics" | "match";

export interface LeftDockProps {
  client: EditorClient;
  active: LeftWorkspace;
  onChange: (workspace: LeftWorkspace) => void;
  onContextMenu: (id: string, x: number, y: number) => void;
  onStartPipe: () => void;
  onImport: () => void;
  onCollapse?: () => void;
  onPin?: () => void;
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
      <span aria-hidden="true">{pinning ? "⌑" : "‹"}</span>
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
        <span aria-hidden="true">!</span>
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

export function LeftDock({ client, active, onChange, onContextMenu, onStartPipe, onImport, onCollapse, onPin }: LeftDockProps) {
  const entities = useEntityOrder();
  const tabs = [
    { id: "scene", label: "Scene", icon: "▱", badge: entities.length, tooltip: "Navigate, select and organize scene objects" },
    { id: "create", label: "Create", icon: "＋", tooltip: "Add local assets and procedural geometry" },
  ] as const;
  return (
    <WorkspacePanel
      data-testid="left-dock"
      title={active === "scene" ? "Scene" : "Create"}
      subtitle={active === "scene" ? `${entities.length.toLocaleString("en-GB")} objects` : "Local tools and asset library"}
      actions={active === "scene" || onCollapse || onPin ? (
        <>
          {active === "scene" && <NeedsAttentionPopup />}
          {(onCollapse || onPin) && <DockChromeAction label="left" onCollapse={onCollapse} onPin={onPin} />}
        </>
      ) : undefined}
      actionsLabel="Scene and dock actions"
      scroll={false}
      tabs={<DockTabs id="left-workspaces" ariaLabel="Scene workspaces" tabs={tabs} activeId={active} onChange={(id) => onChange(id as LeftWorkspace)} />}
    >
      <div
        id="left-workspaces-scene-panel"
        role="tabpanel"
        aria-labelledby="left-workspaces-scene-tab"
        hidden={active !== "scene"}
        className="mtk-dock-panel"
      >
        <AuthoringToolbar client={client} />
        <div className="mtk-dock-panel__fill"><Hierarchy client={client} onContextMenu={onContextMenu} /></div>
      </div>
      <div
        id="left-workspaces-create-panel"
        role="tabpanel"
        aria-labelledby="left-workspaces-create-tab"
        hidden={active !== "create"}
        className="mtk-dock-panel mtk-scroll"
      >
        <div className="mtk-quick-create" role="group" aria-label="Quick creation tools">
          <Button data-testid="create-pipe" variant="primary" onClick={onStartPipe} title="Draw a production pipe asset directly in the viewport">
            <span aria-hidden="true">⌁</span> Draw pipe
          </Button>
          <Button data-testid="create-import" variant="secondary" onClick={onImport} title="Choose a supported local asset file">
            <span aria-hidden="true">⇩</span> Import
          </Button>
        </div>
        <DisclosureSection title="Describe" summary="Optional assisted creation" defaultOpen={false} storageKey="create-describe">
          <DescribeBar client={client} />
        </DisclosureSection>
        <div className="mtk-dock-section-heading">Asset library</div>
        <AssetBrowser client={client} />
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
}

export function InspectorDock({ client, active, onChange, onCollapse, onPin }: InspectorDockProps) {
  const selected = useSelectedId();
  const tabs = [
    { id: "properties", label: "Properties", icon: "⚙", tooltip: "Edit the selected object's components and material" },
    { id: "relations", label: "Relations", icon: "⌘", tooltip: "Inspect compatible bindings and graph relationships" },
    { id: "physics", label: "Physics", icon: "◉", tooltip: "Simulation, collision and mechanism controls" },
    { id: "match", label: "Match", icon: "⚔", tooltip: "Author, validate and run this scene as a match" },
  ] as const;
  return (
    <WorkspacePanel
      data-testid="inspector-dock"
      title="Inspector"
      subtitle={selected ? "Selection context" : "Nothing selected"}
      actions={onCollapse || onPin ? <DockChromeAction label="Inspector" onCollapse={onCollapse} onPin={onPin} /> : undefined}
      actionsLabel="Inspector dock layout"
      scroll={false}
      tabs={<DockTabs id="inspector-workspaces" ariaLabel="Inspector workspaces" tabs={tabs} activeId={active} onChange={(id) => onChange(id as InspectorWorkspace)} />}
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
                <DisclosureSection title="Diagnostics" summary="Selection health and quick fixes" defaultOpen storageKey="inspect-diagnostics">
                  <Diagnostics client={client} />
                </DisclosureSection>
                <DisclosureSection title="Material" summary="Local presets and surface appearance" defaultOpen={false} storageKey="inspect-material">
                  <AiEditPanel client={client} />
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
          <EmptyPanelState title="Select an object to inspect its relationships" description="Compatible bindings and the local relationship graph will appear here." icon="⌘" />
        )}
      </div>
      <div
        id="inspector-workspaces-physics-panel"
        role="tabpanel"
        aria-labelledby="inspector-workspaces-physics-tab"
        hidden={active !== "physics"}
        className="mtk-dock-panel mtk-scroll"
      >
        {active === "physics" && <LazyWorkspace label="Physics inspector"><PhysicsPanel client={client} /></LazyWorkspace>}
      </div>
      <div
        id="inspector-workspaces-match-panel"
        role="tabpanel"
        aria-labelledby="inspector-workspaces-match-tab"
        hidden={active !== "match"}
        className="mtk-dock-panel mtk-scroll"
      >
        {active === "match" && <LazyWorkspace label="Match workspace"><MatchPanel client={client} /></LazyWorkspace>}
      </div>
    </WorkspacePanel>
  );
}
