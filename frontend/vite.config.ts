import { defineConfig } from 'vitest/config'
import vue from '@vitejs/plugin-vue'

// https://vite.dev/config/
export default defineConfig({
  plugins: [
    vue({
      template: {
        compilerOptions: {
          // vue-advanced-chat/emoji-picker sont de vrais Web Components,
          // pas des SFC Vue : on dit au compilateur de ne pas essayer de
          // les résoudre comme des composants enregistrés.
          isCustomElement: (tag) => tag === 'vue-advanced-chat' || tag === 'emoji-picker',
        },
      },
    }),
  ],
  test: {
    environment: 'jsdom',
  },
})
