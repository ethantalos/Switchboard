import react from '@vitejs/plugin-react'
import { defineConfig } from 'vite'

// https://vite.dev/config/
export default defineConfig({
  plugins: [react()],
  // Keep Rust compiler output on screen instead of clearing it.
  clearScreen: false,
  server: {
    // tauri.conf.json points devUrl at this exact port, so fail loudly rather
    // than silently moving to another one.
    port: 5173,
    strictPort: true,
    watch: {
      // The Rust build writes into src-tauri/target while Vite is watching,
      // and Windows locks those files. Watching them crashes the dev server.
      ignored: ['**/src-tauri/**'],
    },
  },
})
