import { ref, type Ref } from 'vue';
import type { ApiClient } from '../api/client';
import { ApiError } from '../api/client';

export interface CrudResource<T> {
  items: Ref<T[]>;
  loading: Ref<boolean>;
  error: Ref<string | null>;
  fetch: () => Promise<void>;
  create: (body: unknown) => Promise<T>;
  update: (id: string, body: unknown) => Promise<T>;
  remove: (id: string) => Promise<void>;
}

/** Fetch/loading/error + CRUD sur `basePath`, factorisant le pattern répété dans
 *  les écrans Settings (Skills, McpServers, LlmProviders, ModelProfiles, Toolsets,
 *  Agents). `create`/`update` propagent leurs erreurs (le call site les route vers
 *  son propre `creationError`/`editError` de dialog) ; `fetch`/`remove` les
 *  capturent dans `error` (affichage dans la card d'erreur partagée de la liste),
 *  à l'identique du comportement actuel de ces écrans. */
export function useCrudResource<T>(client: ApiClient, basePath: string): CrudResource<T> {
  const items = ref<T[]>([]) as Ref<T[]>;
  const loading = ref(true);
  const error = ref<string | null>(null);

  async function fetchAll(): Promise<void> {
    try {
      items.value = await client.get<T[]>(basePath);
    } catch (e) {
      error.value = e instanceof ApiError ? e.message : String(e);
    } finally {
      loading.value = false;
    }
  }

  async function create(body: unknown): Promise<T> {
    const created = await client.post<T>(basePath, body);
    await fetchAll();
    return created;
  }

  async function update(id: string, body: unknown): Promise<T> {
    const updated = await client.put<T>(`${basePath}/${id}`, body);
    await fetchAll();
    return updated;
  }

  async function remove(id: string): Promise<void> {
    try {
      await client.delete(`${basePath}/${id}`);
      await fetchAll();
    } catch (e) {
      error.value = e instanceof ApiError ? e.message : String(e);
    }
  }

  return {
    items, loading, error, fetch: fetchAll, create, update, remove,
  };
}
