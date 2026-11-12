import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// The Tauri CLI owns the Rust build. Vite only serves and bundles the webview UI.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: "127.0.0.1",
    watch: {
      ignored: ["**/src-tauri/**"],
    },
  },
  build: {
    target: ["es2021", "chrome105", "safari13"],
    minify: "esbuild",
    sourcemap: false,
  },
});
