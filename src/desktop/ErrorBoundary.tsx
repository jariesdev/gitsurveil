/**
 * Catches render-phase crashes inside one desktop view.
 *
 * Without a boundary, any uncaught error during a view's render makes React
 * unmount the whole tree and the window goes blank white — exactly what a
 * botched IPC payload (e.g. an old daemon answering a newer method shape) used
 * to do. Wrapping the `<main>` content means a broken view shows its error
 * inline instead, while the sidebar stays usable so the user can navigate
 * away. The boundary is keyed by the current view, so switching views resets
 * it automatically.
 */

import { Component } from "react";
import type { ErrorInfo, ReactNode } from "react";

interface Props {
  children: ReactNode;
  /** Clears the boundary (e.g. "Back to dashboard"). */
  onReset: () => void;
}

interface State {
  error: Error | null;
}

export class ViewErrorBoundary extends Component<Props, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo): void {
    console.error("Desktop view crashed:", error, info);
  }

  render(): ReactNode {
    if (this.state.error) {
      return (
        <div className="flex h-full flex-col items-center justify-center gap-3 p-10 text-center">
          <p className="text-sm font-medium">This view failed to render.</p>
          <p className="max-w-md break-words text-xs text-neutral-500">
            {String(this.state.error)}
          </p>
          <button
            type="button"
            onClick={() => {
              this.setState({ error: null });
              this.props.onReset();
            }}
            className="rounded bg-neutral-900 px-3 py-1.5 text-sm text-white dark:bg-neutral-100 dark:text-neutral-900"
          >
            Back to dashboard
          </button>
        </div>
      );
    }
    return this.props.children;
  }
}
