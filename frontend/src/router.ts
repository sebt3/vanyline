import { createRouter, createWebHistory, type Router, type RouteRecordRaw } from 'vue-router';
import IdeShell from './components/IdeShell.vue';
import SettingsView from './components/SettingsView.vue';
import HomeDashboard from './components/dashboards/HomeDashboard.vue';
import ProjectDashboard from './components/dashboards/ProjectDashboard.vue';
import SandboxOverview from './components/sandbox/SandboxOverview.vue';
import RunDetail from './components/workflows/RunDetail.vue';
import { LG_MIN_WIDTH } from './components/dashboards/viewport';

/** Largeur minimale de l'IDE (breakpoint `lg`, frontend.sdd) : source unique dans viewport.ts. */
export const IDE_MIN_WIDTH = LG_MIN_WIDTH;

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
  {
    path: '/p/:projectName/s/:sandboxName/overview',
    name: 'sandbox-overview',
    component: SandboxOverview,
    props: true,
  },
  {
    path: '/p/:projectName/runs/:runId',
    name: 'run',
    component: RunDetail,
    props: (route) => ({ runId: String(route.params.runId) }),
  },
  { path: '/settings', name: 'settings', component: SettingsView },
];

/** Garde de route : l'IDE (bureau seulement) sur une fenêtre plus étroite que `lg`
 *  redirige vers la vue de sandbox responsive de la même sandbox. `width` est
 *  injectable pour les tests. */
export function installRouteGuards(
  r: Router,
  width: () => number = () => window.innerWidth,
): void {
  r.beforeEach((to) => {
    if (to.name === 'ide' && width() < IDE_MIN_WIDTH) {
      return { name: 'sandbox-overview', params: to.params };
    }
    return true;
  });
}

export const router = createRouter({
  history: createWebHistory(),
  routes,
});

installRouteGuards(router);
