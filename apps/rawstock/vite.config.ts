import { svelte } from "@sveltejs/vite-plugin-svelte";
import { defineConfig } from "vite";

export default defineConfig({
  plugins: [svelte()],
  clearScreen: false,
  server: {
    host: "127.0.0.1",
    port: 1430,
    strictPort: true,
  },
  build: {
    outDir: "dist",
    target: "es2022",
    emptyOutDir: true,
  },
});
