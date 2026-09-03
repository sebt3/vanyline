import type { BridgeClient } from './bridge';
import type { ConfigDomain, ConfigRepo } from '@vanyline/ui';

/**
 * Implémentation RPC de `ConfigRepo` (cf. `@vanyline/ui/ports.ts`) au-dessus du
 * pont postMessage webview→host (`BridgeClient.request` : résout `result`,
 * rejette `BridgeRpcError`). Contrat : `docs/rpc-protocol.md`.
 *
 * Contenu `item`/`patch` snake_case natif, enveloppes d'écriture camelCase ;
 * le `source` additif des lectures est conservé tel quel (badge de couche) et
 * retiré des payloads d'écriture. Jamais de `layer` : on hérite du défaut F2
 * (« pas de sélecteur de couche en v1 »).
 */

/** Domaine UI → méthode RPC de lecture LISTE (seul point de traduction
 *  profiles↔models / mcp↔mcpServers — cf. docs/rpc-protocol.md « camelCase vs
 *  snake_case » : « le domaine RPC s'appelle models … F4 traduit »). */
const LIST_METHOD: Record<ConfigDomain, string> = {
  providers: 'config/providers',
  profiles: 'config/models',
  mcp: 'config/mcpServers',
  toolsets: 'config/toolsets',
  agents: 'config/agents',
  skills: 'config/skills',
};

/** Domaine UI → préfixe des méthodes d'écriture (`/create|/update|/delete`).
 *  Mêmes valeurs que `LIST_METHOD` ; table séparée pour que la distinction
 *  lecture/écriture reste explicite à la lecture du code. */
const WRITE_PREFIX: Record<ConfigDomain, string> = {
  providers: 'config/providers',
  profiles: 'config/models',
  mcp: 'config/mcpServers',
  toolsets: 'config/toolsets',
  agents: 'config/agents',
  skills: 'config/skills',
};

/** Ligne de config côté wire : objet de domaine snake_case, contenu non vérifié
 *  au runtime (JSON traversant le pont) — d'où le cast unique en sortie. */
type Row = Record<string, unknown>;

export function createRpcConfigRepo(bridge: BridgeClient): ConfigRepo {
  async function list(domain: ConfigDomain): Promise<Row[]> {
    return bridge.request<Row[]>(LIST_METHOD[domain], {});
  }

  /** `skills` a une lecture détail dédiée (`config/skills/get`, seul endroit
   *  où le `body` est exposé) ; les autres domaines se relisent dans la liste
   *  (le RPC n'a pas de lecture unité par nom). */
  async function get(domain: ConfigDomain, name: string): Promise<Row> {
    if (domain === 'skills') {
      return bridge.request<Row>('config/skills/get', { name });
    }
    const items = await list(domain);
    const found = items.find((item) => item.name === name);
    if (found === undefined) {
      throw new Error(`VNL-EXT-023: ${domain}/${name} introuvable`);
    }
    return found;
  }

  async function create(domain: ConfigDomain, item: Row): Promise<Row> {
    // `source` : additif de lecture RPC (badge), jamais sur le wire d'écriture
    // — retiré explicitement plutôt que compter sur la tolérance serde.
    const { source, ...payload } = item;
    if (domain === 'skills') {
      // Exception de forme skills (rpc-protocol) : `item` ne porte QUE
      // name/description, le `body` voyage dans l'enveloppe (sinon VNL-RPC-015).
      await bridge.request(`${WRITE_PREFIX.skills}/create`, {
        item: { name: payload.name, description: payload.description },
        body: payload.body,
      });
    } else {
      await bridge.request(`${WRITE_PREFIX[domain]}/create`, { item: payload });
    }
    // Le succès d'écriture répond `result: null` — relire : le serveur est la
    // source de vérité (« seule la lecture renvoie l'entrée »).
    return get(domain, item.name as string);
  }

  async function update(domain: ConfigDomain, name: string, patch: Row): Promise<Row> {
    const { source, ...clean } = patch;
    // skills : `description` et/ou `body` patchés tels quels (le contrat RPC
    // patche ces clés — rpc-protocol « Exception de forme pour skills »).
    await bridge.request(`${WRITE_PREFIX[domain]}/update`, { name, patch: clean });
    return get(domain, name);
  }

  async function remove(domain: ConfigDomain, name: string): Promise<void> {
    await bridge.request(`${WRITE_PREFIX[domain]}/delete`, { name });
  }

  const repo = {
    list,
    get,
    create,
    update,
    remove,

    /** `is_default` provider = concept web-only (`app`), absent du CLI :
     *  documenté dans le port (« Impl RPC → rejet ») — jamais de requête. */
    async setDefaultProvider(_name: string): Promise<void> {
      throw new Error(
        'VNL-EXT-024: fournisseur par défaut non supporté côté CLI (concept web-only)',
      );
    },

    async testProvider(name: string): Promise<{ models: string[] }> {
      return bridge.request<{ models: string[] }>('config/providers/test', { name });
    },

    async testMcpServer(name: string): Promise<{ tools: string[] }> {
      return bridge.request<{ tools: string[] }>('config/mcpServers/test', { name });
    },

    /** Le RPC renvoie des descripteurs MCP complets {name, description,
     *  inputSchema} ; le port attend juste les noms. */
    async listLocalTools(): Promise<string[]> {
      const tools = await bridge.request<Array<{ name: string }>>('config/localTools', {});
      return tools.map((t) => t.name);
    },
  };

  // Cast unique (comme `httpConfigRepo`) : l'impl travaille en `ConfigDomain`
  // + `Row`, alors que les génériques de `ConfigRepo` sont par domaine et que
  // l'asymétrie list/get sur skills (SkillMeta sans body ↔ SkillDetail avec
  // body) n'est pas exprimable par domaine au niveau d'une seule signature.
  return repo as unknown as ConfigRepo;
}
