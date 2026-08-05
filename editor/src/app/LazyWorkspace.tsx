//! Shared production boundary for editor workspaces loaded on demand.

import { Component, Suspense, type ErrorInfo, type ReactNode } from "react";
import { Button } from "../theme/primitives";

interface WorkspaceErrorBoundaryProps {
  children: ReactNode;
  label: string;
}

interface WorkspaceErrorBoundaryState {
  failed: boolean;
}

class WorkspaceErrorBoundary extends Component<WorkspaceErrorBoundaryProps, WorkspaceErrorBoundaryState> {
  state: WorkspaceErrorBoundaryState = { failed: false };

  static getDerivedStateFromError(): WorkspaceErrorBoundaryState {
    return { failed: true };
  }

  componentDidCatch(error: unknown, info: ErrorInfo) {
    console.error(`Failed to load ${this.props.label}`, error, info.componentStack);
  }

  render() {
    if (this.state.failed) {
      return (
        <div className="mtk-workspace-state" role="alert" data-testid="workspace-load-error">
          <strong>{this.props.label} could not be loaded</strong>
          <span>The editor is still safe to use. Reload to retry this workspace.</span>
          <Button variant="secondary" compact onClick={() => window.location.reload()}>
            Reload editor
          </Button>
        </div>
      );
    }
    return this.props.children;
  }
}

export function LazyWorkspace({ label, children }: { label: string; children: ReactNode }) {
  return (
    <WorkspaceErrorBoundary label={label}>
      <Suspense
        fallback={(
          <div className="mtk-workspace-state" role="status" aria-live="polite" data-testid="workspace-loading">
            <span className="mtk-spinner" aria-hidden="true" />
            <span>Loading {label.toLocaleLowerCase("en-GB")}…</span>
          </div>
        )}
      >
        {children}
      </Suspense>
    </WorkspaceErrorBoundary>
  );
}
