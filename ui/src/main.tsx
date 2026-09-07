import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { BrowserRouter } from "react-router-dom";
import App from "./App";
import "./styles.css";
import { tunnel } from "./tunnel";

// Attached through a relay (docs/RELAY.md §11)? Bring the tunnel up
// before the first request; requests await it.
if (tunnel.enabled) void tunnel.start().catch(() => {});

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <BrowserRouter>
      <App />
    </BrowserRouter>
  </StrictMode>,
);
