import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';

// `pnpm dev` talks to an engine started with `blimey … --listen 127.0.0.1:7340`.
const engine = process.env.BLIMEY_ENGINE ?? 'http://127.0.0.1:7340';

export default defineConfig({
  plugins: [svelte()],
  server: {
    proxy: {
      '/api': engine,
      '/ws': { target: engine, ws: true },
    },
  },
});
