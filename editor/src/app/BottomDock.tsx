//! Task/output workspaces live below the viewport instead of competing with the contextual Inspector.
//! The dock is one click away, collapsible, keyboard-tabbed, and remains interactive in Play mode.

import { lazy, useState } from "react";
import { Button } from "../theme/primitives";
import { DockTabs, EmptyPanelState, MenuPopup, PopupMenuItem } from "../theme/workspace";
import { space } from "../theme/tokens";
import { useSelectedId } from "../store/projection";
import type { EditorClient } from "../transport/session";
import { LazyWorkspace } from "./LazyWorkspace";

const AssetLabWorkspace = lazy(() => import("../panels/AssetLabWorkspace").then((module) => ({ default: module.AssetLabWorkspace })));
const Diagnostics = lazy(() => import("../panels/Diagnostics").then((module) => ({ default: module.Diagnostics })));
const ImportReport = lazy(() => import("../panels/ImportReport").then((module) => ({ default: module.ImportReport })));
const ReimportPanel = lazy(() => import("../panels/ReimportPanel").then((module) => ({ default: module.ReimportPanel })));
const RuleDebugPanel = lazy(() => import("../panels/RuleDebugPanel").then((module) => ({ default: module.RuleDebugPanel })));
const RulesPanel = lazy(() => import("../panels/RulesPanel").then((module) => ({ default: module.RulesPanel })));
const StateGraphPanel = lazy(() => import("../panels/StateGraphPanel").then((module) => ({ default: module.StateGraphPanel })));
const ComposePanel = lazy(() => import("../panels/ComposePanel").then((module) => ({ default: module.ComposePanel })));
const BindingGraph = lazy(() => import("../graph/BindingGraph").then((module) => ({ default: module.BindingGraph })));
const AnimationWorkspace = lazy(() => import("../panels/AnimationWorkspace").then((module) => ({ default: module.AnimationWorkspace })));

export type BottomWorkspace = "asset" | "animation" | "problems" | "import" | "logic" | "runtime";
type LogicWorkspace = "rules" | "states" | "graph" | "compose";

export interface BottomDockProps {
  client: EditorClient;
  active: BottomWorkspace;
  open: boolean;
  playing: boolean;
  onChange: (workspace: BottomWorkspace) => void;
  onOpenChange: (open: boolean) => void;
}

export function BottomDock({ client, active, open, playing, onChange, onOpenChange }: BottomDockProps) {
  const selected = useSelectedId();
  const [logic, setLogic] = useState<LogicWorkspace>("rules");
  const tabs = [
    { id: "asset", label: "Asset Lab", icon: "A", tooltip: "Inspect, repair, optimize, finish, validate and export the selected mesh" },
    { id: "animation", label: "Animation", icon: "◆", tooltip: "Key entity properties, preview deterministic motion and inspect rig readiness" },
    { id: "problems", label: "Problems", icon: "!", tooltip: "Selection diagnostics and actionable issues" },
    { id: "import", label: "Import", icon: "⇩", tooltip: "CAD fidelity, re-import changes and repair actions" },
    { id: "logic", label: "Logic", icon: "◇", tooltip: "Rules, state machines, binding graphs and composition" },
    { id: "runtime", label: "Runtime", icon: "▶", badge: playing ? "live" : undefined, tooltip: "Interactive rule and simulation diagnostics during Play" },
  ] as const;
  const activeTab = tabs.find((tab) => tab.id === active) ?? tabs[0];

  function select(id: string) {
    const next = id as BottomWorkspace;
    onChange(next);
  }

  return (
    <section className={`mtk-bottom-dock${open ? " is-open" : ""}${active === "asset" ? " is-asset" : ""}${active === "animation" ? " is-animation" : ""}`} data-testid="bottom-dock" aria-label="Task and output workspaces">
      <div className="mtk-bottom-dock__bar">
        {open ? (
          <DockTabs id="bottom-workspaces" ariaLabel="Task workspaces" tabs={tabs} activeId={active} onChange={select} />
        ) : (
          <MenuPopup
            id="bottom-workspaces-picker"
            label="Choose task workspace"
            placement="top-start"
            trigger={(
              <>
                <span aria-hidden="true">{activeTab.icon}</span>
                <span>{activeTab.label}</span>
                {"badge" in activeTab && activeTab.badge != null && <span className="mtk-dock-tab__badge">{activeTab.badge}</span>}
                <span aria-hidden="true">^</span>
              </>
            )}
            triggerProps={{
              "data-testid": "bottom-workspace-summary",
              variant: "ghost",
              compact: true,
              title: `Current workspace: ${activeTab.label}. Choose another workspace`,
            }}
          >
            {(close) => tabs.map((tab) => (
              <PopupMenuItem
                key={tab.id}
                data-testid={`bottom-workspace-option-${tab.id}`}
                label={tab.label}
                description={tab.tooltip}
                leading={tab.icon}
                meta={tab.id === active ? "Current" : ("badge" in tab ? tab.badge : undefined)}
                variant={tab.id === active ? "toggle" : "ghost"}
                active={tab.id === active}
                aria-current={tab.id === active ? "page" : undefined}
                onSelect={() => select(tab.id)}
                onRequestClose={close}
              />
            ))}
          </MenuPopup>
        )}
        <Button
          data-testid="bottom-dock-toggle"
          variant="ghost"
          compact
          icon
          aria-label={open ? "Collapse task workspace" : "Expand task workspace"}
          aria-expanded={open}
          aria-controls={`bottom-workspaces-${active}-panel`}
          onClick={() => onOpenChange(!open)}
          title={open ? "Collapse" : "Expand"}
        >
          <span aria-hidden="true">{open ? "⌄" : "⌃"}</span>
        </Button>
      </div>
      <div hidden>
        {tabs
          .filter((tab) => !open || tab.id !== active)
          .map((tab) => (
            <div
              key={tab.id}
              id={`bottom-workspaces-${tab.id}-panel`}
              role="tabpanel"
              aria-labelledby={`bottom-workspaces-${tab.id}-tab`}
            />
          ))}
      </div>
      {open && (
        <div className="mtk-bottom-dock__content">
          {active === "asset" && (
            <div id="bottom-workspaces-asset-panel" role="tabpanel" aria-labelledby="bottom-workspaces-asset-tab" className="mtk-bottom-workspace mtk-scroll">
              <LazyWorkspace label="Asset Lab"><AssetLabWorkspace client={client} /></LazyWorkspace>
            </div>
          )}
          {active === "animation" && (
            <div id="bottom-workspaces-animation-panel" role="tabpanel" aria-labelledby="bottom-workspaces-animation-tab" className="mtk-bottom-workspace mtk-scroll">
              <LazyWorkspace label="Animation"><AnimationWorkspace client={client} /></LazyWorkspace>
            </div>
          )}
          {active === "problems" && (
            <div id="bottom-workspaces-problems-panel" role="tabpanel" aria-labelledby="bottom-workspaces-problems-tab" className="mtk-bottom-workspace mtk-scroll">
              {selected ? <LazyWorkspace label="Problems"><Diagnostics client={client} /></LazyWorkspace> : <EmptyPanelState compact icon="✓" title="No selection-specific problems" description="Select an object to inspect actionable diagnostics." />}
            </div>
          )}
          {active === "import" && (
            <div id="bottom-workspaces-import-panel" role="tabpanel" aria-labelledby="bottom-workspaces-import-tab" className="mtk-bottom-workspace mtk-bottom-workspace--split mtk-scroll">
              <LazyWorkspace label="Import tools">
                <div><ReimportPanel client={client} /></div>
                <div><ImportReport client={client} /></div>
              </LazyWorkspace>
            </div>
          )}
          {active === "logic" && (
            <LogicWorkspacePanel client={client} active={logic} onChange={setLogic} />
          )}
          {active === "runtime" && (
            <div id="bottom-workspaces-runtime-panel" role="tabpanel" aria-labelledby="bottom-workspaces-runtime-tab" className="mtk-bottom-workspace mtk-scroll">
              {playing ? <LazyWorkspace label="Runtime diagnostics"><RuleDebugPanel client={client} /></LazyWorkspace> : <EmptyPanelState compact icon="▶" title="Runtime tools become live in Play" description="Press Play to inspect rule decisions without disabling this workspace." />}
            </div>
          )}
        </div>
      )}
    </section>
  );
}

function LogicWorkspacePanel({ client, active, onChange }: { client: EditorClient; active: LogicWorkspace; onChange: (workspace: LogicWorkspace) => void }) {
  const tabs = [
    { id: "rules", label: "Rules" },
    { id: "states", label: "State machines" },
    { id: "graph", label: "Binding graph" },
    { id: "compose", label: "Compose", tooltip: "Optional assisted composition" },
  ] as const;
  return (
    <div id="bottom-workspaces-logic-panel" role="tabpanel" aria-labelledby="bottom-workspaces-logic-tab" className="mtk-bottom-workspace mtk-bottom-workspace--logic">
      <DockTabs ariaLabel="Logic editors" tabs={tabs} activeId={active} onChange={(id) => onChange(id as LogicWorkspace)} />
      <div className="mtk-bottom-workspace__body mtk-scroll" style={{ padding: space.sm }}>
        <LazyWorkspace label="Logic editor">
          {active === "rules" && <RulesPanel client={client} />}
          {active === "states" && <StateGraphPanel client={client} />}
          {active === "graph" && <BindingGraph />}
          {active === "compose" && <ComposePanel client={client} />}
        </LazyWorkspace>
      </div>
    </div>
  );
}
