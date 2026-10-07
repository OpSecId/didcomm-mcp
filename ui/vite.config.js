import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';

// `npm run dev` proxies the API to a server on :8090 (didcomm-mcp --http).
export default defineConfig({
  plugins: [svelte()],
  server: {
    proxy: {
      '/api': 'http://127.0.0.1:8090',
      '/healthz': 'http://127.0.0.1:8090',
    },
  },
  build: { outDir: 'dist', emptyOutDir: true, sourcemap: false },
});
