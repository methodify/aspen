import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";
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

// Dev proxy: /api (REST + WebSocket) → the aspen node daemon.
export default defineConfig({
  plugins: [react()],
  define: {
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
