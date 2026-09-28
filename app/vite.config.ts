import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Основная сборка интерфейса PowerBench для Tauri.
// `outDir` обязан совпадать с `build.frontendDist` в
// `src-tauri/tauri.conf.json` (`../build-ui`): при расхождении Tauri
// упаковывает устаревший интерфейс или не находит его вовсе.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  build: {
    outDir: "build-ui",
    emptyOutDir: true,
    target: "chrome105",
  },
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      ignored: ["**/src-tauri/**"],
    },
  },
});
