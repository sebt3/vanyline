import { defineConfig, type Plugin } from 'vite';
import vue from '@vitejs/plugin-vue';
import ui from '@nuxt/ui/vite';
import { fileURLToPath, URL } from 'node:url';

// Vite 8 (rolldown) émet le CSS unique sous assets/style.css (defaultCssBundleName),
// jamais index.css. Le contrat dur (et buildHtml) exigent assets/index.css :
// last-writer plugin qui renomme l'asset CSS après render.
const cssAsIndex: Plugin = {
  name: 'css-as-index',
  enforce: 'post',
  generateBundle(_options, bundle) {
    const asset = bundle['assets/style.css'];
    if (!asset) return;
    delete bundle['assets/style.css'];
    this.emitFile({ type: 'asset', fileName: 'assets/index.css', source: asset.source });
  },
};

export default defineConfig({
  root: fileURLToPath(new URL('.', import.meta.url)),
  plugins: [vue(), ui(), cssAsIndex],
  resolve: {
    alias: {
      '@vanyline/ui': fileURLToPath(new URL('../../packages/ui/src', import.meta.url)),
      '@vanyline/protocol': fileURLToPath(new URL('../../packages/protocol/src', import.meta.url)),
    },
  },
  build: {
    outDir: fileURLToPath(new URL('../dist/webview', import.meta.url)),
    emptyOutDir: true,
    cssCodeSplit: false,
    rollupOptions: {
      output: {
        entryFileNames: 'assets/index.js',
        assetFileNames: 'assets/[name][extname]',
        // CSP du buildHtml : script-src 'nonce-…' sans strict-dynamic → un chunk
        // importé dynamiquement serait bloqué. Tout doit tenir dans index.js.
        inlineDynamicImports: true,
      },
    },
  },
});
