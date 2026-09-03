import { createApp } from 'vue';
import ui from '@nuxt/ui/vue-plugin';
import './style.css';
import App from './App.vue';

document.documentElement.classList.add('dark');
createApp(App).use(ui).mount('#app');
