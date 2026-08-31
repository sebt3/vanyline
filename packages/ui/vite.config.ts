import { defineConfig } from 'vitest/config';
import vue from '@vitejs/plugin-vue';
import ui from '@nuxt/ui/vite';
import { fileURLToPath, URL } from 'node:url';

export default defineConfig({
  plugins: [vue(), ui()],
  resolve: {
    alias: {
      '@vanyline/protocol': fileURLToPath(new URL('../protocol/src', import.meta.url)),
      // Alias @nuxt/ui/dist/runtime → @nuxt/ui/runtime so vitest resolves
      // ChatMessages.vue etc. via the package exports map (`./runtime/*`).
      '@nuxt/ui/dist/runtime': fileURLToPath(new URL('../../node_modules/@nuxt/ui/dist/runtime', import.meta.url)),
    },
  },
  test: {
    environment: 'jsdom',
    setupFiles: ['./src/test-setup.ts'],
  },
});