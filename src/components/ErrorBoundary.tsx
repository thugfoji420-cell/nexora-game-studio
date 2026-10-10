import { Component, ErrorInfo, ReactNode } from "react";

interface Props {
  children: ReactNode;
  fallback?: ReactNode;
}

interface State {
  hasError: boolean;
  error: Error | null;
  errorInfo: ErrorInfo | null;
}

export class ErrorBoundary extends Component<Props, State> {
  state: State = { hasError: false, error: null, errorInfo: null };

  static getDerivedStateFromError(error: Error): Partial<State> {
    return { hasError: true, error };
  }

  componentDidCatch(error: Error, errorInfo: ErrorInfo) {
    console.error("[NEXORA] ErrorBoundary caught:", error, errorInfo);
    this.setState({ error, errorInfo });
  }

  render() {
    if (this.state.hasError) {
      if (this.props.fallback) {
        return this.props.fallback;
      }
      const err = this.state.error;
      const info = this.state.errorInfo;
      return (
        <div style={{ padding: '40px', color: '#ff6b6b', textAlign: 'center', fontFamily: 'monospace' }}>
          <h1>React Error</h1>
          <p>Something went wrong during rendering.</p>
          <details style={{ textAlign: 'left', maxWidth: '800px', margin: '20px auto', padding: '16px', background: '#1a1a1a', borderRadius: '8px', overflow: 'auto' }}>
            <summary style={{ cursor: 'pointer', fontWeight: 'bold', marginBottom: '12px' }}>Error Details (click to expand)</summary>
            <pre style={{ whiteSpace: 'pre-wrap', wordBreak: 'break-word', color: '#ffaaaa' }}>
              {err?.message ?? 'Unknown error'}
              {err?.stack && `\n\nStack:\n${err.stack}`}
            </pre>
            {info?.componentStack && (
              <>
                <hr style={{ margin: '16px 0', borderColor: '#333' }} />
                <pre style={{ whiteSpace: 'pre-wrap', wordBreak: 'break-word', color: '#aaa999', fontSize: '12px' }}>
                  Component Stack:\n{info.componentStack}
                </pre>
              </>
            )}
          </details>
          <button
            onClick={() => this.setState({ hasError: false, error: null, errorInfo: null })}
            style={{ marginTop: '16px', padding: '10px 20px', background: '#4da6ff', border: 'none', borderRadius: '4px', color: '#000', fontWeight: 'bold', cursor: 'pointer' }}
          >
            Try Again
          </button>
        </div>
      );
    }
    return this.props.children;
  }
}