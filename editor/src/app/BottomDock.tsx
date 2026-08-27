//! Task/output workspaces live below the viewport instead of competing with the contextual Inspector.
//! The dock is one click away, collapsible, keyboard-tabbed, and remains interactive in Play mode.

import { lazy, useState } from "react";
import { Icon } from "../theme/icons";
import { Button } from "../theme/primitives";
import { DockTabs, EmptyPanelState, MenuPopup, PopupMenuItem } from "../theme/workspace";
import { space } from "../theme/tokens";
import { useSelectedId } from "../store/projection";
import type { EditorClient } from "../transport/session";
import type { DockForm } from "./layout";
import { LazyWorkspace } from "./LazyWorkspace";

const AssetLabWorkspace = lazy(() => import("../panels/AssetLabWorkspace").then((module) => ({ default: module.AssetLabWorkspace })));
const Diagnostics = lazy(() => import("../panels/Diagnostics").then((module) => ({ default: module.Diagnostics })));
const ImportReport = lazy(() => import("../panels/ImportReport").then((module) => ({ default: module.ImportReport })));
const FormatsPanel = lazy(() => import("../panels/FormatsPanel").then((module) => ({ default: module.FormatsPanel })));
const ReimportPanel = lazy(() => import("../panels/ReimportPanel").then((module) => ({ default: module.ReimportPanel })));
const RuleDebugPanel = lazy(() => import("../panels/RuleDebugPanel").then((module) => ({ default: module.RuleDebugPanel })));
const RulesPanel = lazy(() => import("../panels/RulesPanel").then((module) => ({ default: module.RulesPanel })));
const StateGraphPanel = lazy(() => import("../panels/StateGraphPanel").then((module) => ({ default: module.StateGraphPanel })));
const ComposePanel = lazy(() => import("../panels/ComposePanel").then((module) => ({ default: module.ComposePanel })));
const BindingGraph = lazy(() => import("../graph/BindingGraph").then((module) => ({ default: module.BindingGraph })));
const AnimationWorkspace = lazy(() => import("../panels/AnimationWorkspace").then((module) => ({ default: module.AnimationWorkspace })));

export type BottomWorkspace = "asset" | "animation" | "problems" | "import" | "logic" | "runtime" | "formats";
type LogicWorkspace = "rules" | "states" | "graph" | "compose";

export interface BottomDockProps {
  client: EditorClient;
  active: BottomWorkspace;
  open: boolean;
  /** A track below the stage, or — on a window too short for both — a sheet over it. See `dockForm`. */
  form?: DockForm;
  playing: boolean;
  onChange: (workspace: BottomWorkspace) => void;
  onOpenChange: (open: boolean) => void;
}

export function BottomDock({ client, active, open, form = "docked", playing, onChange, onOpenChange }: BottomDockProps) {
  const selected = useSelectedId();
  const [logic, setLogic] = useState<LogicWorkspace>("rules");
  // Labels and icons match the Engines rail EXACTLY, and the sub-engines come first in the rail's order.
  // Calling the same thing "Model" on the rail and "Asset Lab" here was the old confusion in miniature:
  // one capability, two names, two places. Problems and Runtime sit after a divider because they are
  // diagnostics, not sub-engines — they are about the state of the world, not a thing you author.
  const tabs = [
    { id: "asset", label: "Model", icon: "model", tooltip: "Repair, optimise and export meshes" },
    { id: "import", label: "Import", icon: "import", tooltip: "CAD fidelity and re-import" },
    { id: "formats", label: "Formats", icon: "meld", tooltip: "What this build can read and write" },
    { id: "animation", label: "Animate", icon: "animate", tooltip: "Timelines, rigs and motion" },
    { id: "logic", label: "Logic", icon: "logic", tooltip: "Rules, states and bindings" },
    { id: "problems", label: "Problems", icon: "problems", tooltip: "Selection diagnostics and actionable issues" },
    { id: "runtime", label: "Runtime", icon: "runtime", badge: playing ? "live" : undefined, tooltip: "Live rule and simulation diagnostics during Play" },
  ] as const;
  const activeTab = tabs.find((tab) => tab.id === active) ?? tabs[0];

  function select(id: string) {
    const next = id as BottomWorkspace;
    onChange(next);
  }

  return (
    // `is-sheet` states the FORM, and the stylesheet spends it only on `.is-open`: a closed dock is
    // its 42px bar, it fits at every height there is, and floating it would cover the stage to say
    // nothing. `data-dock-form` is the stable token the layout gate keys on — a class list is styling
    // and drifts, and asserting on `position: absolute` would test the mechanism instead of the state.
    <section className={`mtk-bottom-dock${open ? " is-open" : ""}${form === "sheet" ? " is-sheet" : ""}${active === "asset" ? " is-asset" : ""}${active === "animation" ? " is-animation" : ""}`} data-testid="bottom-dock" data-dock-form={form} aria-label="Task and output workspaces">
      <div className="mtk-bottom-dock__bar">
        {open ? (
          <DockTabs id="bottom-workspaces" ariaLabel="Task workspaces" tabs={tabs.map((tab) => ({ ...tab, icon: <Icon name={tab.icon} size="md" /> }))} activeId={active} onChange={select} />
        ) : (
          <MenuPopup
            id="bottom-workspaces-picker"
            label="Choose task workspace"
            placement="top-start"
            trigger={(
              <>
                <Icon name={activeTab.icon} size="md" />
                <span>{activeTab.label}</span>
                {"badge" in activeTab && activeTab.badge != null && <span className="mtk-dock-tab__badge">{activeTab.badge}</span>}
                <Icon name="chevron-up" size="sm" />
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
                leading={<Icon name={tab.icon} size="md" />}
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
          <Icon name={open ? "chevron-down" : "chevron-up"} size="md" />
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
              <LazyWorkspace label="Model"><AssetLabWorkspace client={client} /></LazyWorkspace>
            </div>
          )}
          {active === "animation" && (
            <div id="bottom-workspaces-animation-panel" role="tabpanel" aria-labelledby="bottom-workspaces-animation-tab" className="mtk-bottom-workspace mtk-scroll">
              <LazyWorkspace label="Animation"><AnimationWorkspace client={client} /></LazyWorkspace>
            </div>
          )}
          {active === "problems" && (
            <div id="bottom-workspaces-problems-panel" role="tabpanel" aria-labelledby="bottom-workspaces-problems-tab" className="mtk-bottom-workspace mtk-scroll">
              {selected ? <LazyWorkspace label="Problems"><Diagnostics client={client} /></LazyWorkspace> : <EmptyPanelState compact icon={<Icon name="check" size="xl" />} title="No selection-specific problems" description="Select an object to inspect actionable diagnostics." />}
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
          {active === "formats" && (
            <div id="bottom-workspaces-formats-panel" role="tabpanel" aria-labelledby="bottom-workspaces-formats-tab" className="mtk-bottom-workspace mtk-scroll">
              <LazyWorkspace label="Supported formats">
                <FormatsPanel client={client} />
              </LazyWorkspace>
            </div>
          )}
          {active === "logic" && (
            <LogicWorkspacePanel client={client} active={logic} onChange={setLogic} />
          )}
          {active === "runtime" && (
            <div id="bottom-workspaces-runtime-panel" role="tabpanel" aria-labelledby="bottom-workspaces-runtime-tab" className="mtk-bottom-workspace mtk-scroll">
              {playing ? <LazyWorkspace label="Runtime diagnostics"><RuleDebugPanel client={client} /></LazyWorkspace> : <EmptyPanelState compact icon={<Icon name="runtime" size="xl" />} title="Runtime tools become live in Play" description="Press Play to inspect rule decisions without disabling this workspace." />}
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
  const fullBleed = active === "states";
  return (
    <div id="bottom-workspaces-logic-panel" role="tabpanel" aria-labelledby="bottom-workspaces-logic-tab" className="mtk-bottom-workspace mtk-bottom-workspace--logic">
      <DockTabs ariaLabel="Logic editors" tabs={tabs} activeId={active} onChange={(id) => onChange(id as LogicWorkspace)} />
      {/* An editor that owns the whole panel anatomy — header, rail, fixed footer — fills the body
          itself: the wrapper stops padding and stops scrolling, so ITS footer stays pinned instead of
          scrolling away inside a second scroll region. The other three are content in a padded well. */}
      <div
        className={fullBleed ? "mtk-bottom-workspace__body is-bleed" : "mtk-bottom-workspace__body mtk-scroll"}
        style={fullBleed ? undefined : { padding: space.sm }}
      >
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
