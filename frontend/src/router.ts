import { createRouter, createWebHistory, type RouteRecordRaw } from 'vue-router';
import IdeShell from './components/IdeShell.vue';
import SettingsView from './components/SettingsView.vue';
import HomeDashboard from './components/dashboards/HomeDashboard.vue';
import ProjectDashboard from './components/dashboards/ProjectDashboard.vue';

/** Routes du shell. Exportées séparément pour que les tests construisent un
 *  routeur mémoire (`createMemoryHistory`) avec les mêmes routes. */
export const routes: RouteRecordRaw[] = [
  { path: '/', name: 'home', component: HomeDashboard },
  { path: '/p/:projectName', name: 'project', component: ProjectDashboard, props: true },
  {
    path: '/p/:projectName/s/:sandboxName',
    name: 'ide',
    component: IdeShell,
    props: true,
  },
  { path: '/settings', name: 'settings', component: SettingsView },
];

export const router = createRouter({
  history: createWebHistory(),
  routes,
});
