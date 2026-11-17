import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

const API_ORIGIN = process.env.JD_API_ORIGIN ?? "http://localhost:8080";

// The dev server proxies the API so the session cookie stays same-origin.
// `ws` carries the web terminal.
export default defineConfig({
  plugins: [react(), tailwindcss()],
  server: { port: 5173, proxy: { "/api/": { target: API_ORIGIN, ws: true } } },
});
