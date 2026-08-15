import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

export default defineConfig({
  plugins: [react(), tailwindcss()],
  // Tauri expects a fixed port and fails the dev command if it's taken,
  // rather than silently serving the app somewhere the webview isn't looking.
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      ignored: ["**/target/**"],
    },
  },
  build: {
    outDir: "dist",
    emptyOutDir: true,
  },
  test: {
    environment: "jsdom",
    globals: true,
    setupFiles: ["./src/test-setup.ts"],
  },
});
