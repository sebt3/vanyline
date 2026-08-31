import { ref, type Ref } from 'vue';
import type { ConfigDomain, ConfigItem, ConfigListItem, ConfigRepo } from '../ports';

/**
 * Fetch / loading / error + CRUD sur un domaine de `ConfigRepo`, factorisant le
 * pattern répété dans les écrans de configuration.
 *
 * Équivalent name-keyed du `useCrudResource` de `frontend/` (qui, lui, reste
 * `(client, basePath)` pour les dashboards Projects/Sandboxes). Ici la clé est
 * le `name` ; l'id interne éventuel du backend ne transite jamais.
 *
 * `create` / `update` **propagent** leurs erreurs (le call site les route vers
 * son propre message de dialog) ; `fetch` / `remove` les capturent dans `error`
 * (carte d'erreur de la liste) — comportement identique aux écrans actuels.
 */
export interface CrudResource<D extends ConfigDomain> {
  items: Ref<ConfigListItem<D>[]>;
  loading: Ref<boolean>;
  error: Ref<string | null>;
  fetch: () => Promise<void>;
  create: (item: ConfigItem<D>) => Promise<ConfigItem<D>>;
  update: (name: string, patch: Partial<ConfigItem<D>>) => Promise<ConfigItem<D>>;
  remove: (name: string) => Promise<void>;
}

function message(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

export function useCrudResource<D extends ConfigDomain>(
  repo: ConfigRepo,
  domain: D,
): CrudResource<D> {
  const items = ref<ConfigListItem<D>[]>([]) as Ref<ConfigListItem<D>[]>;
  const loading = ref(true);
  const error = ref<string | null>(null);

  async function fetchAll(): Promise<void> {
    try {
      items.value = await repo.list(domain);
    } catch (e) {
      error.value = message(e);
    } finally {
      loading.value = false;
    }
  }

  async function create(item: ConfigItem<D>): Promise<ConfigItem<D>> {
    const created = await repo.create(domain, item);
    await fetchAll();
    return created;
  }

  async function update(name: string, patch: Partial<ConfigItem<D>>): Promise<ConfigItem<D>> {
    const updated = await repo.update(domain, name, patch);
    await fetchAll();
    return updated;
  }

  async function remove(name: string): Promise<void> {
    try {
      await repo.remove(domain, name);
      await fetchAll();
    } catch (e) {
      error.value = message(e);
    }
  }

  return { items, loading, error, fetch: fetchAll, create, update, remove };
}
