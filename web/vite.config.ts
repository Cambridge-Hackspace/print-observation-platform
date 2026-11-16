import { defineConfig } from 'vitest/config';
import { svelte } from '@sveltejs/vite-plugin-svelte';

// The app is built into dist/, which the server compiles into its binary
// (crates/pop-server/src/assets.rs). `npm run dev` serves it with hot reload
// and passes the API through to a server on POP_URL (http://127.0.0.1:8080
// unless set).
const api = process.env.POP_URL ?? 'http://127.0.0.1:8080';

export default defineConfig({
  plugins: [svelte()],
  build: { outDir: 'dist', emptyOutDir: true },
  server: {
    proxy: {
      '/api': { target: api, changeOrigin: true },
      '/healthz': { target: api, changeOrigin: true },
    },
  },
  test: {
    include: ['src/**/*.test.ts'],
    environment: 'node',
  },
});
