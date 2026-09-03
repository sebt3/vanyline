import { createApp } from 'vue';
import ui from '@nuxt/ui/vue-plugin';
import './style.css';
import App from './App.vue';
import ConfigView from './ConfigView.vue';
import { resolveView } from './router';

document.documentElement.classList.add('dark');

// Une webview VS Code n'a pas de window.location.search : la vue est portée par
// la balise <meta name="vanyline-view"> que buildHtml émet (chat | config).
const meta = document
  .querySelector('meta[name="vanyline-view"]')
  ?.getAttribute('content');
const root = resolveView(meta) === 'config' ? ConfigView : App;
createApp(root).use(ui).mount('#app');

