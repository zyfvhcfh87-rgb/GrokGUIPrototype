import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

const developmentHost = "127.0.0.1";

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    host: developmentHost,
    port: 1420,
    strictPort: true,
    watch: {
      ignored: ["**/src-tauri/**", "**/target/**"],
    },
  },
  preview: {
    host: developmentHost,
    port: 1420,
    strictPort: true,
  },
  build: {
    target: "es2022",
  },
});
