import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { BrowserRouter, HashRouter } from "react-router-dom";
import App from "./App";
import "./styles.css";
import "./phone.css";
import { tunnel } from "./tunnel";
import { pwa } from "./pwa";

// Attached through a relay (docs/RELAY.md §11)? Bring the tunnel up
// before the first request; requests await it.
if (tunnel.enabled) void tunnel.start().catch(() => {});
pwa.start();

// Hosted (methodify.github.io/aspen): a static host has no SPA fallback,
// so routes live in the hash. Served by a node: real paths.
const Router = __ASPEN_HOSTED__ ? HashRouter : BrowserRouter;

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <Router>
      <App />
    </Router>
  </StrictMode>,
);
