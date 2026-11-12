import { Component, type ReactNode } from "react";

interface MessageBoundaryProps {
  children: ReactNode;
  text: string;
  resetKey: string;
}

interface MessageBoundaryState {
  failed: boolean;
}

/**
 * Markdown rendering and model output are untrusted. If a reply still manages
 * to throw while rendering, show its raw text instead of unmounting the whole
 * application. The boundary resets when the surrounding message changes.
 */
export class MessageBoundary extends Component<
  MessageBoundaryProps,
  MessageBoundaryState
> {
  state: MessageBoundaryState = { failed: false };

  static getDerivedStateFromError(): MessageBoundaryState {
    return { failed: true };
  }

  componentDidCatch(error: unknown) {
    console.error("A chat message could not be rendered.", error);
  }

  componentDidUpdate(previous: MessageBoundaryProps) {
    if (this.state.failed && previous.resetKey !== this.props.resetKey) {
      this.setState({ failed: false });
    }
  }

  render() {
    if (this.state.failed) {
      return (
        <div className="rounded-xl border border-rose-400/20 bg-rose-400/5 px-3 py-2">
          <p className="text-[10px] font-medium uppercase tracking-wider text-rose-300/80">
            This reply could not be formatted
          </p>
          <pre className="mt-2 max-h-72 overflow-auto whitespace-pre-wrap break-words text-[11.5px] leading-5 text-slate-300">
            {this.props.text || "The model returned an empty message."}
          </pre>
        </div>
      );
    }
    return this.props.children;
  }
}
