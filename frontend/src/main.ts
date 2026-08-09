import { createApp } from 'vue';
import { register as registerAdvancedChat } from 'vue-advanced-chat';
import './style.css';
import App from './App.vue';

registerAdvancedChat();

createApp(App).mount('#app');
