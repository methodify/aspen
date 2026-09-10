import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";
import { VitePWA } from "vite-plugin-pwa";
import { readFileSync } from "node:fs";
import { execSync } from "node:child_process";

// The UI's version is the workspace version — one number, bumped in one
// place (Cargo.toml) — so the console can tell when it's stale relative to
// the daemon serving it.
function workspaceVersion(): string {
  try {
    const toml = readFileSync(new URL("../Cargo.toml", import.meta.url), "utf8");
    return /^version\s*=\s*"([^"]+)"/m.exec(toml)?.[1] ?? "0.0.0";
  } catch {
    return "0.0.0";
  }
}
function gitSha(): string {
  try {
    return execSync("git rev-parse --short HEAD", { stdio: ["ignore", "pipe", "ignore"] })
      .toString()
      .trim();
  } catch {
    return "unknown";
  }
}

// Hosted build (PROPOSALS-2026-09-D.md §3): the console as a static site
// at methodify.github.io/aspen — served from a sub-path, hash-routed,
// connecting to nodes and relays it is pointed at.
const hosted = process.env.VITE_HOSTED === "1";
const base = hosted ? "/aspen/" : "/";
const route = (p: string) => (hosted ? `${base}#${p}` : p);

// Dev proxy: /api (REST + WebSocket) → the aspen node daemon.
export default defineConfig({
  base,
  plugins: [
    react(),
    // Installable, and a shell that survives the node (§1–2): the app
    // shell is precached; /api is never cached; a new build asks for a
    // reload rather than taking over.
    VitePWA({
      registerType: "prompt",
      // Our own worker (src/sw.ts): the precache plus the push handler.
      strategies: "injectManifest",
      srcDir: "src",
      filename: "sw.ts",
      injectManifest: {
        globPatterns: ["**/*.{js,css,html,svg,png,woff2}"],
      },
      includeAssets: ["aspen-mark.svg", "icons/*.png"],
      manifest: {
        name: "Aspen",
        short_name: "Aspen",
        description: "A control plane for fleets of coding agents",
        start_url: route("/"),
        scope: base,
        display: "standalone",
        background_color: "#0e141b",
        theme_color: "#0e141b",
        icons: [
          { src: "icons/aspen-192.png", sizes: "192x192", type: "image/png" },
          { src: "icons/aspen-512.png", sizes: "512x512", type: "image/png" },
          { src: "icons/aspen-maskable-192.png", sizes: "192x192", type: "image/png", purpose: "maskable" },
          { src: "icons/aspen-maskable-512.png", sizes: "512x512", type: "image/png", purpose: "maskable" },
          { src: "aspen-mark.svg", sizes: "any", type: "image/svg+xml" },
        ],
        shortcuts: [
          { name: "Now", url: route("/"), description: "the fleet right now" },
          { name: "Search", url: route("/search"), description: "search every transcript" },
          { name: "Boards", url: route("/boards"), description: "your boards" },
        ],
        protocol_handlers: [{ protocol: "web+aspen", url: route("/open?u=%s") }],
      },
    }),
  ],
  define: {
    __ASPEN_HOSTED__: JSON.stringify(hosted),
    __ASPEN_UI_VERSION__: JSON.stringify(workspaceVersion()),
    __ASPEN_UI_SHA__: JSON.stringify(gitSha()),
  },
  server: {
    proxy: {
      "/api": {
        target: "http://127.0.0.1:7420",
        ws: true,
      },
    },
  },
  test: {
    environment: "node",
    include: ["src/**/*.test.ts"],
  },
});
