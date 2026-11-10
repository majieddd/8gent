import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// In a plain browser (npm run dev) the page reaches the service through this proxy; inside the
// Tauri window it goes through the app's own `api` command, so the service needs no CORS rules.
const service = `http://127.0.0.1:${process.env.EIGHTGENT_PORT ?? 8436}`;

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    proxy: { "/8gent": service, "/v1": service },
  },
  build: { target: "es2022", outDir: "dist" },
});
