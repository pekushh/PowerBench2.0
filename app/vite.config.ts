import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Настройка Vite для фронтенда Tauri-приложения PowerBench.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      ignored: ["**/src-tauri/**"],
    },
  },
});