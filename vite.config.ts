import { defineConfig, loadEnv } from "vite";
import react from "@vitejs/plugin-react";
// @ts-expect-error node builtin, resolved by vite's own runtime
import { fileURLToPath } from "node:url";

// @ts-expect-error process is a nodejs global
const host = process.env.TAURI_DEV_HOST;

/** Absolute path to an HTML entry point, next to this config file. */
const entry = (name: string): string => fileURLToPath(new URL(name, import.meta.url));

// https://vite.dev/config/
export default defineConfig(({ mode }) => {
  // @ts-expect-error process is a nodejs global
  const env = loadEnv(mode, process.cwd(), "");

  /**
   * The mock backend is opt-in and resolved here, at build time, not at
   * runtime: `--mode mock` (or VITE_ONYX_MOCK=1) inlines `true`, anything else
   * inlines `false` so `src/lib/mock.ts` is tree-shaken out of the bundle.
   */
  const mock = mode === "mock" || env.VITE_ONYX_MOCK === "1";

  return {
    plugins: [react()],

    define: {
      __ONYX_MOCK__: JSON.stringify(mock),
    },

    // Vite options tailored for Tauri development and only applied in
    // `tauri dev` or `tauri build`
    //
    // 1. prevent Vite from obscuring rust errors
    clearScreen: false,
    // 2. tauri expects a fixed port, fail if that port is not available
    server: {
      port: 1420,
      strictPort: true,
      host: host || false,
      hmr: host
        ? {
            protocol: "ws",
            host,
            port: 1421,
          }
        : undefined,
      watch: {
        // 3. tell Vite to ignore watching `src-tauri`
        ignored: ["**/src-tauri/**"],
      },
    },

    build: {
      // the webviews Tauri targets are evergreen; keep the output small
      target: "es2022",
      sourcemap: false,
      rollupOptions: {
        /**
         * Three documents, one bundle. `eq.html` is the detached EQ window of
         * SPEC §12 and `theme.html` the theme editor of SPEC §20 — both real
         * webviews, loaded by Rust as `WebviewUrl::App(…)`, so they have to be
         * emitted next to `index.html` in `dist/` (and in `dist-mock/`, where
         * the mock preview opens them as popups). Shared modules — `lib/eq.ts`,
         * `lib/themedoc.ts`, the canvas plumbing, the store — are hoisted into
         * a common chunk by Rollup rather than duplicated into each entry.
         */
        input: {
          main: entry("index.html"),
          eq: entry("eq.html"),
          theme: entry("theme.html"),
        },
      },
    },
  };
});
