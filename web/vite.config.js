import { defineConfig } from "vite";
import { viteSingleFile } from "vite-plugin-singlefile";

// Single self-contained HTML (JS+CSS inlined) → served by the ESP32 from LittleFS.
// JSX in classic mode, mapped to the module-local `React` (= preact/compat).
export default defineConfig({
  plugins: [viteSingleFile()],
  esbuild: {
    jsx: "transform",
    jsxFactory: "React.createElement",
    jsxFragment: "React.Fragment",
  },
  build: {
    target: "es2020",
    cssCodeSplit: false,
    assetsInlineLimit: 100000000,
    chunkSizeWarningLimit: 100000,
  },
});
