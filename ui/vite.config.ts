import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

// Dev proxy: /api (REST + WebSocket) → the aspen node daemon.
export default defineConfig({
  plugins: [react()],
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
