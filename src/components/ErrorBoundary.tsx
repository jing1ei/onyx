/**
 * The last line of defence: a render error anywhere in the tree used to blank
 * the window with no message, no log line and no way back except quitting the
 * app. React does not report these anywhere by itself.
 */

import { Component, type ErrorInfo, type ReactNode } from "react";
import { logError } from "../lib/log";

interface Props {
  children: ReactNode;
}

interface State {
  error: Error | null;
}

export default class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo): void {
    logError(`react render error: ${error.message}`, `${error.stack ?? ""}${info.componentStack ?? ""}`);
  }

  render(): ReactNode {
    const { error } = this.state;
    if (!error) return this.props.children;
    return (
      <div className="fatal">
        <strong>Onyx hit an unexpected error</strong>
        <span className="num">{error.message}</span>
        <span>
          Playback is unaffected — the engine runs outside this window. The failure has been written
          to the log.
        </span>
        <button className="solid-btn" onClick={() => window.location.reload()}>
          Reload the window
        </button>
      </div>
    );
  }
}
