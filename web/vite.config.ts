import { defineConfig } from "vite";
import { readFileSync } from "node:fs";

const { version } = JSON.parse(readFileSync(new URL("./package.json", import.meta.url), "utf8"));

export default defineConfig({
  define: { __PORTAL_VERSION__: JSON.stringify(version) },
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
