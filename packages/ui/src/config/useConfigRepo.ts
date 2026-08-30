import { inject } from 'vue';
import type { ConfigRepo } from '../ports';

/** Clé d'injection du `ConfigRepo` (fourni par l'embarqueur : `httpConfigRepo`
 *  côté web, impl RPC côté extension VS Code en F4). */
export const CONFIG_REPO_KEY = 'vanyline.configRepo';

export function useConfigRepo(): ConfigRepo {
  const repo = inject<ConfigRepo>(CONFIG_REPO_KEY);
  if (!repo) {
    throw new Error(`VNL-UI-001 ConfigRepo non fourni — provide('${CONFIG_REPO_KEY}', …)`);
  }
  return repo;
}
