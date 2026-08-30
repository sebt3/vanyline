import { ApiError, createApiClient } from './client';
import type {
  ConfigDomain,
  ConfigRepo,
} from '@vanyline/ui';
import type { PagedResult } from '../composables/useCrudResource';

/** Mapping domaine UI → endpoint REST `app` (seul point de traduction des
 *  noms de domaines côté web ; `profiles` (UI) = `model-profiles` (app)). */
const ENDPOINTS: Record<ConfigDomain, string> = {
  providers: '/api/v1/llm-providers',
  profiles: '/api/v1/model-profiles',
  mcp: '/api/v1/mcp-servers',
  toolsets: '/api/v1/toolsets',
  agents: '/api/v1/agents',
  skills: '/api/v1/skills',
};

type Row = Record<string, unknown>;

function nonEmptyObject(v: unknown): v is Record<string, unknown> {
  return !!v && typeof v === 'object' && Object.keys(v as object).length > 0;
}

/**
 * Implémentation HTTP de `ConfigRepo` (cf. `@vanyline/ui/ports.ts`).
 *
 * Porte **toute** la traduction entre le wire REST de `app` (PK `i32`,
 * `provider_type`/`server_type`, FK `provider_id`/`model_profile_id`) et la
 * forme canonique `@vanyline/protocol/config-domain.ts` (name-keyed,
 * discriminant `type`, miroir de `lib/src/domain.rs`). L'impl RPC de F4 sera
 * un pass-through — c'est ici que vit l'asymétrie.
 *
 * Caches `name↔id` **par instance** (pas de partage entre `httpConfigRepo()`
 * successifs — l'id interne ne sort jamais du repo).
 */
export function httpConfigRepo(): ConfigRepo {
  const client = createApiClient();
  const nameToId = new Map<ConfigDomain, Map<string, number>>();
  const idToName = new Map<ConfigDomain, Map<number, string>>();

  /** Listing dépaginé (pattern `PagedResult`, cf. `useCrudResource`) + (ré)alimentation
   *  des caches `name↔id` du domaine. Renvoie les lignes REST brutes. */
  async function fetchRows(domain: ConfigDomain): Promise<Row[]> {
    const base = ENDPOINTS[domain];
    const first = await client.get<Row[] | PagedResult<Row>>(base);
    let rows: Row[];
    if (Array.isArray(first)) {
      rows = first;
    } else {
      rows = [...first.items];
      for (let page = first.page + 1; page <= first.total_pages; page += 1) {
        const sep = base.includes('?') ? '&' : '?';
        const next = await client.get<Row[] | PagedResult<Row>>(`${base}${sep}page=${page}`);
        rows.push(...(Array.isArray(next) ? next : next.items));
      }
    }
    const n2i = new Map<string, number>();
    const i2n = new Map<number, string>();
    for (const r of rows) {
      if (typeof r.id === 'number' && typeof r.name === 'string') {
        n2i.set(r.name, r.id);
        i2n.set(r.id, r.name);
      }
    }
    nameToId.set(domain, n2i);
    idToName.set(domain, i2n);
    return rows;
  }

  async function ensure(domain: ConfigDomain): Promise<void> {
    if (!nameToId.has(domain)) await fetchRows(domain);
  }

  /** Les domaines qui portent une FK ont besoin du cache du domaine référencé
   *  pour traduire `*_id` → nom (sens lecture). */
  async function ensureRefs(domain: ConfigDomain): Promise<void> {
    if (domain === 'profiles') await ensure('providers');
    if (domain === 'agents') await ensure('profiles');
  }

  async function idOf(domain: ConfigDomain, name: string): Promise<number> {
    await ensure(domain);
    const id = nameToId.get(domain)?.get(name);
    if (id === undefined) {
      throw new ApiError(404, undefined, `VNL-CFG-404 ${domain}/${name} introuvable`);
    }
    return id;
  }

  function nameOf(domain: ConfigDomain, id: unknown): string {
    if (typeof id !== 'number') return '';
    return idToName.get(domain)?.get(id) ?? String(id);
  }

  // -------------------------------------------------------------------------
  // REST → canonique
  // -------------------------------------------------------------------------

  function fromRest(domain: ConfigDomain, r: Row): Row {
    switch (domain) {
      case 'providers':
        return {
          name: r.name,
          type: r.provider_type,
          endpoint: r.endpoint,
          ...(r.api_key != null ? { api_key: r.api_key } : {}),
          available_models: r.available_models ?? [],
          is_default: r.is_default ?? false,
        };
      case 'profiles':
        return {
          name: r.name,
          provider: nameOf('providers', r.provider_id),
          model: r.model,
          ...(r.temperature != null ? { temperature: r.temperature } : {}),
          ...(r.max_tokens != null ? { max_tokens: r.max_tokens } : {}),
          ...(nonEmptyObject(r.options) ? { options: r.options } : {}),
        };
      case 'mcp':
        return {
          name: r.name,
          type: r.server_type,
          url: r.url,
          ...(nonEmptyObject(r.headers) ? { headers: r.headers } : {}),
          available_tools: r.available_tools ?? [],
        };
      case 'toolsets':
        return {
          name: r.name,
          ...(r.description != null ? { description: r.description } : {}),
          ...(r.prompt != null ? { prompt: r.prompt } : {}),
          local_tools: r.local_tools ?? [],
          mcp: r.mcp ?? [],
        };
      case 'agents':
        return {
          name: r.name,
          ...(r.description != null ? { description: r.description } : {}),
          mode: r.mode,
          model: nameOf('profiles', r.model_profile_id),
          toolsets: r.toolsets ?? [],
          skills: r.skills ?? 'auto',
          system_prompt: r.system_prompt ?? '',
        };
      case 'skills':
        return {
          name: r.name,
          description: r.description ?? '',
          ...(r.body !== undefined ? { body: r.body } : {}),
        };
    }
  }

  // -------------------------------------------------------------------------
  // canonique → REST (async : profiles/agents résolvent une FK nom → id)
  // -------------------------------------------------------------------------

  async function toRest(domain: ConfigDomain, item: Row): Promise<Row> {
    const has = (k: string): boolean => Object.prototype.hasOwnProperty.call(item, k);
    const out: Row = {};
    switch (domain) {
      case 'providers':
        if (has('name')) out.name = item.name;
        if (has('type')) out.provider_type = item.type;
        if (has('endpoint')) out.endpoint = item.endpoint;
        if (has('api_key')) out.api_key = item.api_key;
        return out;
      case 'profiles':
        if (has('name')) out.name = item.name;
        if (has('provider')) out.provider_id = await idOf('providers', item.provider as string);
        if (has('model')) out.model = item.model;
        if (has('temperature')) out.temperature = item.temperature;
        if (has('max_tokens')) out.max_tokens = item.max_tokens;
        if (has('options')) out.options = item.options;
        return out;
      case 'mcp':
        if (has('name')) out.name = item.name;
        if (has('type')) out.server_type = item.type;
        if (has('url')) out.url = item.url;
        if (has('headers')) out.headers = item.headers;
        return out;
      case 'toolsets':
        for (const k of ['name', 'description', 'prompt', 'local_tools', 'mcp']) {
          if (has(k)) out[k] = item[k];
        }
        return out;
      case 'agents':
        if (has('name')) out.name = item.name;
        if (has('description')) out.description = item.description;
        if (has('mode')) out.mode = item.mode;
        if (has('model')) out.model_profile_id = await idOf('profiles', item.model as string);
        if (has('toolsets')) out.toolsets = item.toolsets;
        if (has('skills')) out.skills = item.skills;
        if (has('system_prompt')) out.system_prompt = item.system_prompt;
        return out;
      case 'skills':
        for (const k of ['name', 'description', 'body']) {
          if (has(k)) out[k] = item[k];
        }
        return out;
    }
  }

  // -------------------------------------------------------------------------
  // Repo (les génériques de `ConfigRepo` sont satisfaits par un cast unique —
  // l'impl travaille en `ConfigDomain`/`Row`, l'asymétrie list/get sur skills
  // est portée par `fromRest`).
  // -------------------------------------------------------------------------

  const repo = {
    async list(domain: ConfigDomain): Promise<Row[]> {
      await ensureRefs(domain);
      const rows = await fetchRows(domain);
      return rows.map((r) => {
        const canonical = fromRest(domain, r);
        if (domain === 'skills') delete canonical.body;
        return canonical;
      });
    },

    async get(domain: ConfigDomain, name: string): Promise<Row> {
      await ensureRefs(domain);
      const id = await idOf(domain, name);
      const row = await client.get<Row>(`${ENDPOINTS[domain]}/${id}`);
      return fromRest(domain, row);
    },

    async create(domain: ConfigDomain, item: Row): Promise<Row> {
      await ensureRefs(domain);
      const body = await toRest(domain, { ...item });
      const created = await client.post<Row>(ENDPOINTS[domain], body);
      await fetchRows(domain);
      return fromRest(domain, created);
    },

    async update(domain: ConfigDomain, name: string, patch: Row): Promise<Row> {
      await ensureRefs(domain);
      const id = await idOf(domain, name);
      const body = await toRest(domain, { ...patch });
      const updated = await client.put<Row>(`${ENDPOINTS[domain]}/${id}`, body);
      await fetchRows(domain);
      return fromRest(domain, updated);
    },

    async remove(domain: ConfigDomain, name: string): Promise<void> {
      const id = await idOf(domain, name);
      await client.delete(`${ENDPOINTS[domain]}/${id}`);
      await fetchRows(domain);
    },

    async setDefaultProvider(name: string): Promise<void> {
      const id = await idOf('providers', name);
      await client.put(`/api/v1/llm-providers/${id}/default`);
      await fetchRows('providers');
    },

    async testProvider(name: string): Promise<{ models: string[] }> {
      const id = await idOf('providers', name);
      return client.post<{ models: string[] }>(`/api/v1/llm-providers/${id}/test`);
    },

    async testMcpServer(name: string): Promise<{ tools: string[] }> {
      const id = await idOf('mcp', name);
      return client.post<{ tools: string[] }>(`/api/v1/mcp-servers/${id}/test`);
    },

    async listLocalTools(): Promise<string[]> {
      const rows = await client.get<Array<{ name: string; description: string }>>('/api/local-tools');
      return rows.map((t) => t.name);
    },
  };

  return repo as unknown as ConfigRepo;
}
