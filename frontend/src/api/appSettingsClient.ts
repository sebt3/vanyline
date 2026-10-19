import { createApiClient } from './client';
import type { ApiClient } from './client';

/** Clé du réglage « modèle par défaut » (app/src/db/entities/app_settings.sdd). */
export const DEFAULT_MODEL_KEY = 'default_model';

/** Groupe administrateur de miryad-core (`ADMIN_GROUP_NAME`, « admin »). */
export const ADMIN_GROUP = 'admin';

/** Valeur du réglage `default_model` : fournisseur global, nom du modèle, options facultatives. */
export interface DefaultModelValue {
  provider_id: number;
  model: string;
  options?: Record<string, unknown>;
}

export interface ProviderOption {
  id: number;
  name: string;
}

interface AppSettingRow {
  id: number;
  key: string;
  value: unknown;
}

type Paged<T> = T[] | { items: T[]; page: number; total_pages: number };

async function listAll<T>(client: ApiClient, base: string): Promise<T[]> {
  const first = await client.get<Paged<T>>(base);
  if (Array.isArray(first)) return first;
  const rows = [...first.items];
  for (let page = first.page + 1; page <= first.total_pages; page += 1) {
    const next = await client.get<Paged<T>>(`${base}?page=${page}`);
    rows.push(...(Array.isArray(next) ? next : next.items));
  }
  return rows;
}

/**
 * Vrai si le principal courant appartient au groupe `admin` (`GET /api/v1/me` de miryad-core).
 * Toute erreur (route absente, réponse non JSON, réseau) donne `false` : le groupe Global
 * reste masqué plutôt que d'être proposé à tort.
 */
export async function isAdmin(client: ApiClient = createApiClient()): Promise<boolean> {
  try {
    const me = await client.get<{ groups?: unknown }>('/api/v1/me');
    return Array.isArray(me.groups) && me.groups.includes(ADMIN_GROUP);
  } catch {
    return false;
  }
}

/** Fournisseurs LLM proposés pour le modèle par défaut. */
export async function listProviderOptions(client: ApiClient = createApiClient()): Promise<ProviderOption[]> {
  const rows = await listAll<Record<string, unknown>>(client, '/api/v1/llm-providers');
  return rows
    .filter((r) => typeof r.id === 'number' && typeof r.name === 'string')
    .map((r) => ({ id: r.id as number, name: r.name as string }));
}

/** Ligne `default_model` existante, ou `null` si elle n'a pas encore été créée. */
export async function getDefaultModel(
  client: ApiClient = createApiClient(),
): Promise<{ id: number; value: DefaultModelValue } | null> {
  const rows = await listAll<AppSettingRow>(client, '/api/v1/app-settings');
  const row = rows.find((r) => r.key === DEFAULT_MODEL_KEY);
  return row ? { id: row.id, value: row.value as DefaultModelValue } : null;
}

/** POST à la création de la ligne `default_model`, PUT ensuite. */
export async function saveDefaultModel(
  current: { id: number } | null,
  value: DefaultModelValue,
  client: ApiClient = createApiClient(),
): Promise<void> {
  if (current) {
    await client.put<void>(`/api/v1/app-settings/${current.id}`, { key: DEFAULT_MODEL_KEY, value });
  } else {
    await client.post<void>('/api/v1/app-settings', { key: DEFAULT_MODEL_KEY, value });
  }
}
