import { defineConfig } from 'vitest/config';
import vue from '@vitejs/plugin-vue';
import ui from '@nuxt/ui/vite';
import { fileURLToPath, URL } from 'node:url';

export default defineConfig({
  plugins: [vue(), ui()],
  resolve: {
    alias: {
      '@vanyline/ui': fileURLToPath(new URL('../packages/ui/src', import.meta.url)),
      '@vanyline/protocol': fileURLToPath(new URL('../packages/protocol/src', import.meta.url)),
    },
  },
  test: {
    include: ['src/**/*.spec.ts', 'webview/src/**/*.spec.ts'],
  },
});
