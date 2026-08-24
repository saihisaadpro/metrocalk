//! The root's last honest sentence.
//!
//! `createSession()` now THROWS in a production bundle with no engine behind it, rather than quietly
//! handing the UI a fake core (see its doc comment in `transport/session.ts`). A throw during render
//! with no boundary above it is a **white page** — React unmounts the tree and says nothing a user can
//! read — which is the silent mock's failure wearing different clothes. This boundary makes the refusal
//! visible: it names what went wrong and offers the one action that can help.
//!
//! It is deliberately the only thing above `App`. Workspace-level failures are already caught much
//! closer to the user by `LazyWorkspace`'s boundary, which keeps the rest of the editor usable; this
//! one exists for the failures that leave nothing to keep.

import { Component, type ErrorInfo, type ReactNode } from "react";
import { Button } from "../theme/primitives";
import { NoCoreError } from "../transport/session";

export class RootBoundary extends Component<{ children: ReactNode }, { error: Error | null }> {
  state: { error: Error | null } = { error: null };

  static getDerivedStateFromError(error: Error) {
    return { error };
  }

  componentDidCatch(error: unknown, info: ErrorInfo) {
    console.error("editor root failed to render", error, info.componentStack);
  }

  render() {
    const { error } = this.state;
    if (!error) return this.props.children;
    // Two different true sentences, because they are two different situations for the person reading
    // them: an engine that never answered is not a drawing bug, and telling someone to reload when the
    // engine is missing is only useful if we also say that is what happened.
    const engineMissing = error instanceof NoCoreError;
    return (
      <div className="mtk-root-failure" role="alert" data-testid="root-failure">
        <strong>
          {engineMissing ? "The editor could not reach the Metrocalk engine." : "The editor could not start."}
        </strong>
        <span>
          {engineMissing
            ? "The window opened but the engine behind it did not answer, so nothing here could have been saved. Restarting Metrocalk usually recovers it."
            : "Something went wrong while drawing the editor. Restarting Metrocalk usually recovers it."}
        </span>
        <Button variant="secondary" onClick={() => window.location.reload()}>
          Reload editor
        </Button>
      </div>
    );
  }
}
