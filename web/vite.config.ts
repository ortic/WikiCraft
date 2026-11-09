import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

const backend = process.env.WIKICRAFT_BACKEND ?? 'http://127.0.0.1:8787'

export default defineConfig({
  plugins: [react()],
  server: {
    port: 5199,
    proxy: {
      '/api': { target: backend, changeOrigin: false, ws: true },
    },
  },
  build: {
    chunkSizeWarningLimit: 2000,
  },
})
