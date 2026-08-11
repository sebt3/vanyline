import { createApp } from 'vue';
import { register as registerAdvancedChat } from 'vue-advanced-chat';
import './style.css';
import App from './App.vue';
import { router } from './router';

registerAdvancedChat();

createApp(App).use(router).mount('#app');
