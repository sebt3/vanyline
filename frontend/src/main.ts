import { createApp } from 'vue';
import { useDark } from '@vueuse/core';
import ui from '@nuxt/ui/vue-plugin';
import './style.css';
import './assets/css/main.css';
import App from './App.vue';
import { router } from './router';

createApp(App).use(router).use(ui).mount('#app');

// `@nuxt/ui/vue-plugin` pose son propre plugin de dark mode (`useDark()` de
// `@vueuse/core`, cf. sa source), qui suit par défaut la préférence système/
// `localStorage` — indépendant du thème sombre fixe du reste du shell IDE
// (dockview-abyss, Element Plus). Si le système n'est pas en dark mode, les
// tokens de couleur Nuxt UI rendent en clair sur notre fond sombre → mauvais
// contraste. L'app n'a jamais eu de mode clair : on force le dark mode plutôt
// que de suivre une préférence système sans rapport avec notre thème.
useDark().value = true;
