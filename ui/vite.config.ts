import { defineConfig } from 'vite'
import { svelte } from '@sveltejs/vite-plugin-svelte'

// Dev server settings follow Tauri's expectations: fixed port matching
// `build.devUrl` in tauri.conf.json, and fail fast if it is taken.
export default defineConfig({
  plugins: [svelte()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
  },
})
