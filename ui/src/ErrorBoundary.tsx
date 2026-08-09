import { Component, type ReactNode } from 'react'
import styles from './ErrorBoundary.module.css'

type Props = { children: ReactNode }
type State = { error: Error | null }

/// The last line of defence around the whole console. `getJson` (data/api.ts)
/// casts JSON without runtime validation, and this phase's screens all iterate
/// what it returns unchecked — valid JSON of the wrong shape (a proxy in front
/// answering with something that parses but isn't the expected DTO, say)
/// throws at render time, past any promise `.catch`. Before this existed that
/// blanked the whole console to a white screen: nothing in the DOM, nothing
/// telling the operator why. This does not retry or recover anything — it
/// exists solely to turn a silent blank page into one that says what broke.
export class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null }

  static getDerivedStateFromError(error: Error): State {
    return { error }
  }

  render() {
    if (this.state.error) {
      return (
        <div className={styles.crashed}>
          <p className={styles.title}>Something went wrong.</p>
          <pre className={styles.detail}>{this.state.error.message}</pre>
        </div>
      )
    }
    return this.props.children
  }
}
