import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import { fixYTiptap } from './scripts/fix-y-tiptap'

const backend = process.env.WIKICRAFT_BACKEND ?? 'http://127.0.0.1:8787'

export default defineConfig({
  plugins: [react(), fixYTiptap()],
  // pre-bundled packages skip the plugins: load y-tiptap as it is, so the fix applies in dev too
  optimizeDeps: { exclude: ['@tiptap/y-tiptap'] },
  server: {
    port: 5199,
    // the icon set is shared with the Rust crates (crates/doc-model/assets/icons.json)
    fs: { allow: ['..'] },
    proxy: {
      '/api': { target: backend, changeOrigin: false, ws: true },
    },
  },
  build: {
    chunkSizeWarningLimit: 2000,
  },
})
