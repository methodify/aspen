import { Component, StrictMode, useEffect, useState, type ReactNode } from "react";
import { createRoot } from "react-dom/client";
import { BrowserRouter, HashRouter } from "react-router-dom";
import App from "./App";
import "./styles.css";
import "./phone.css";
import { tunnel } from "./tunnel";
import { pwa } from "./pwa";
import { followMeshLink, onProfilesChange, switchGeneration } from "./profiles";
import { peek } from "./peek";

// A notification tapped while another mesh is active: switch first
// (docs/PROPOSALS-2026-09-F.md §2.5).
followMeshLink();
window.addEventListener("hashchange", followMeshLink);

// Attached through a relay (docs/RELAY.md §11)? Bring the tunnel up
// before the first request; requests await it.
if (tunnel.enabled) void tunnel.start().catch(() => {});
pwa.start();
peek.start();

/** The app, remounted on a mesh switch (profiles.ts switchGeneration):
 *  every poll and every piece of React state starts over against the
 *  new mesh, and the tunnel restarts with the new profile's identity and
 *  relay — no page reload (F-7). */
function Root() {
  const [gen, setGen] = useState(switchGeneration);
  useEffect(
    () =>
      onProfilesChange(() => {
        const g = switchGeneration();
        if (g === gen) return;
        setGen(g);
        void tunnel.start().catch(() => {});
        peek.restart();
      }),
    [gen],
  );
  return <App key={gen} />;
}

/** Last resort: an error during render shows itself and a way back,
 *  instead of a blank page (the 2026-10 quality pass). */
class Boundary extends Component<{ children: ReactNode }, { error: Error | null }> {
  state = { error: null as Error | null };
  static getDerivedStateFromError(error: Error) {
    return { error };
  }
  render() {
    if (!this.state.error) return this.props.children;
    return (
      <div style={{ padding: 24, fontFamily: "system-ui, sans-serif" }}>
        <h2 style={{ margin: "0 0 8px" }}>Something went wrong in the console</h2>
        <pre style={{ whiteSpace: "pre-wrap", opacity: 0.8 }}>{String(this.state.error.message || this.state.error)}</pre>
        <button onClick={() => { window.location.hash = ""; window.location.reload(); }}>reload</button>
      </div>
    );
  }
}

// Hosted (methodify.github.io/aspen): a static host has no SPA fallback,
// so routes live in the hash. Served by a node: real paths.
const Router = __ASPEN_HOSTED__ ? HashRouter : BrowserRouter;

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <Boundary>
      <Router>
        <Root />
      </Router>
    </Boundary>
  </StrictMode>,
);
