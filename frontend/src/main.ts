import { createApp } from 'vue';
import ui from '@nuxt/ui/vue-plugin';
import './style.css';
import './assets/css/main.css';
import App from './App.vue';
import { router } from './router';

createApp(App).use(router).use(ui).mount('#app');
