import { createRouter, createWebHistory, type RouteRecordRaw } from 'vue-router';
import IdeShell from './components/IdeShell.vue';
import SettingsView from './components/SettingsView.vue';

/** Routes du shell. Exportées séparément pour que les tests construisent un
 *  routeur mémoire (`createMemoryHistory`) avec les mêmes routes. */
export const routes: RouteRecordRaw[] = [
  { path: '/', redirect: '/settings' },
  { path: '/settings', component: SettingsView },
  { path: '/ide/:sandboxName', component: IdeShell, props: true },
];

export const router = createRouter({
  history: createWebHistory(),
  routes,
});
