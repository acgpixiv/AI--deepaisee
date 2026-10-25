import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";

export default defineConfig({
  plugins: [vue()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      ignored: ["**/src-tauri/**", "**/.tools/**", "**/data/**", "**/models/**"],
    },
  },
  build: {
    target: "chrome105",
    minify: "esbuild",
    sourcemap: false,
  },
});
