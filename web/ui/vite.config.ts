/// <reference types="vitest/config" />
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";


export default defineConfig({
  plugins: [react()],
  server: {
    // `yard-web` listens on 8088 by default; the browser only talks to one origin.
    proxy: { "/api": "http://127.0.0.1:8088", "/healthz": "http://127.0.0.1:8088" },
  },
  // No `data:` URIs: the server CSP only allows same-origin files.
  build: { outDir: "dist", sourcemap: false, assetsInlineLimit: 0 },
  test: {
    environment: "jsdom",
    setupFiles: ["./src/test/setup.ts"],
    css: false,
    // Date formatting in tests must not depend on the machine running them.
    env: { TZ: "UTC" },
  },
});
