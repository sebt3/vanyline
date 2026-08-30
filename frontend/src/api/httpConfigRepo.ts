import { ApiError, createApiClient } from './client';
import type { ConfigDomain, ConfigItem, ConfigRepo } from '@vanyline/ui';
import type { PagedResult } from '../composables/useCrudResource';

/** Mapping UI → endpoint REST app (le seul point de traduction des noms de
 *  domaines côté web ; `profiles` (UI) = `model-profiles` (app)). */
const ENDPOINTS: Record<ConfigDomain, string> = {
  providers: '/api/v1/llm-providers',
  profiles: '/api/v1/model-profiles',
  mcp: '/api/v1/mcp-servers',
  toolsets: '/api/v1/toolsets',
  agents: '/api/v1/agents',
  skills: '/api/v1/skills',
};

interface IdRow {
  id: number;
  name: string;
}

export function httpConfigRepo(): ConfigRepo {
  const client = createApiClient();
  // Cache name→id par domaine, alimenté par le listing. L'id interne n'est
  // jamais exposé par le port — seulement utilisé pour router les opérations.
  const ids = new Map<ConfigDomain, Map<string, number>>();
  // Cache des items par domaine (alimenté par list/get).
  const itemsCache = new Map<ConfigDomain, ConfigItem[]>();

  /** Listing dépaginé (pattern PagedResult, cf. useCrudResource) + alimentation
   *  du cache name→id. */
  async function fetchDomain(domain: ConfigDomain): Promise<ConfigItem[]> {
    const base = ENDPOINTS[domain];
    const first = await client.get<ConfigItem[] | PagedResult<ConfigItem>>(base);
    const all = Array.isArray(first) ? first : await fetchPages(base, first);
    const idMap = new Map<string, number>();
    for (const item of all) {
      const id = (item as unknown as IdRow).id;
      const name = item.name;
      if (typeof id === 'number' && name) idMap.set(name, id);
    }
    ids.set(domain, idMap);
    itemsCache.set(domain, all);
    return all;
  }

  /** Pages suivantes d'un PagedResult (dépagination identique à
   *  `useCrudResource.fetchAll`). */
  async function fetchPages(
    base: string,
    first: PagedResult<ConfigItem>,
  ): Promise<ConfigItem[]> {
    const all = [...first.items];
    for (let page = first.page + 1; page <= first.total_pages; page += 1) {
      const sep = base.includes('?') ? '&' : '?';
      const next = await client.get<ConfigItem[] | PagedResult<ConfigItem>>(
        `${base}${sep}page=${page}`,
      );
      all.push(...(Array.isArray(next) ? next : next.items));
    }
    return all;
  }

  /** Résout `name` → `id` (PK interne) pour un domaine, en alimentant le cache
   *  si nécessaire. Lève une erreur si le nom est introuvable. */
  async function idFor(domain: ConfigDomain, name: string): Promise<number> {
    let map = ids.get(domain);
    if (!map) {
      await fetchDomain(domain);
      map = ids.get(domain)!;
    }
    const id = map.get(name);
    if (id === undefined) {
      throw new ApiError(404, undefined, `${domain}/${name} introuvable`);
    }
    return id;
  }

  return {
    async list(domain) {
      return fetchDomain(domain);
    },
    async get(domain, name) {
      const cached = itemsCache.get(domain);
      const items = cached ?? (await fetchDomain(domain));
      const item = items.find((i) => i.name === name);
      if (!item) {
        throw new ApiError(404, undefined, `${domain}/${name} introuvable`);
      }
      return item;
    },
    async create(domain, item) {
      const created = await client.post<ConfigItem>(ENDPOINTS[domain], item);
      await fetchDomain(domain);
      return created;
    },
    async update(domain, name, patch) {
      const id = await idFor(domain, name);
      const updated = await client.put<ConfigItem>(`${ENDPOINTS[domain]}/${id}`, patch);
      await fetchDomain(domain);
      return updated;
    },
    async remove(domain, name) {
      const id = await idFor(domain, name);
      await client.delete(`${ENDPOINTS[domain]}/${id}`);
      await fetchDomain(domain);
    },
    async testProvider(name) {
      const id = await idFor('providers', name);
      return client.post<{ models: string[] }>(`/api/v1/llm-providers/${id}/test`);
    },
    async testMcpServer(name) {
      const id = await idFor('mcp', name);
      return client.post<{ tools: string[] }>(`/api/v1/mcp-servers/${id}/test`);
    },
    async listLocalTools() {
      const rows =
        await client.get<Array<{ name: string; description: string }>>(
          '/api/local-tools',
        );
      return rows.map((t) => t.name);
    },
  };
}
