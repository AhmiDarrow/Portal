import { defineConfig } from "vite";

export default defineConfig({
  server: {
    host: "127.0.0.1",
    proxy: {
      "/api": "http://127.0.0.1:8490",
      "/media": "http://127.0.0.1:8490",
      "/vendor": "http://127.0.0.1:8490",
    },
  },
  build: {
    outDir: "dist",
    emptyOutDir: true,
  },
});
