import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    strictPort: true,
    port: 1420,
    watch: {
      // Preview captures and browser profiles are generated artifacts. They
      // can contain locked Chromium cookie files, which should never be part
      // of Vite's dependency graph or file watcher.
      ignored: ["**/src-tauri/target/**", "**/outputs/**", "**/work/**"],
    },
  },
  envPrefix: ["VITE_", "TAURI_ENV_*"],
  optimizeDeps: {
    entries: ["index.html"],
  },
  build: {
    target: process.env.TAURI_ENV_PLATFORM === "windows" ? "chrome105" : "safari13",
    minify: !process.env.TAURI_ENV_DEBUG ? "esbuild" : false,
    sourcemap: !!process.env.TAURI_ENV_DEBUG,
    rollupOptions: {
      output: {
        manualChunks(id) {
          const modulePath = id.replace(/\\/g, "/");
          if (/\/node_modules\/(react|react-dom|scheduler)\//.test(modulePath)) return "react-vendor";
          if (/\/node_modules\/(framer-motion|motion-dom|motion-utils)\//.test(modulePath)) return "motion-vendor";
          if (modulePath.includes("/node_modules/lucide-react/")) return "icons-vendor";
        },
      },
    },
  },
});
