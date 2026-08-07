// The `latin-` prefix is load-bearing: the unprefixed entrypoints (e.g. `400.css`)
// emit @font-face rules for every Unicode subset fontsource ships (latin, latin-ext,
// cyrillic, cyrillic-ext, greek, vietnamese), and the whole bundle is compiled into
// the Rust binary via rust-embed. The console only ever renders Latin text, so pull
// the latin-only entrypoints to keep the binary from carrying dead weight.
import '@fontsource/ibm-plex-sans/latin-400.css'
import '@fontsource/ibm-plex-sans/latin-500.css'
import '@fontsource/ibm-plex-sans/latin-600.css'
import '@fontsource/ibm-plex-mono/latin-400.css'
import '@fontsource/ibm-plex-mono/latin-500.css'
import '@fontsource/ibm-plex-mono/latin-600.css'
import './theme.css'
import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { App } from './App'
import { ErrorBoundary } from './ErrorBoundary'

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <ErrorBoundary>
      <App />
    </ErrorBoundary>
  </StrictMode>,
)
