import type { Provider, ModelProfile, McpServer, Toolset, Agent, SkillMeta, SkillDetail } from '@vanyline/protocol';

export interface ChatConversation {
  id: string;
  title: string | null;
  createdAt: string;
}

export interface ChatMessageRecord {
  id: string;
  role: 'user' | 'assistant';
  content: string;
}

export interface ChatBackend {
  /** Conversations du sélecteur, plus récentes d'abord. */
  listConversations(): Promise<ChatConversation[]>;
  /** Historique persisté d'une conversation, ordre chronologique. Vide pour une neuve. */
  loadMessages(conversationId: string): Promise<ChatMessageRecord[]>;
  /** Crée une conversation, renvoie son id. L'impl porte son propre contexte
   *  (web = contexte sandbox + résolution du 1ᵉʳ agent ; CLI (F4) = agent du YAML).
   *  Aucun paramètre côté port — la politique de contexte ne remonte jamais
   *  dans les composants. */
  createConversation(): Promise<string>;
}

export type ConfigDomain = 'providers' | 'profiles' | 'mcp' | 'toolsets' | 'agents' | 'skills';

/** Forme d'un item de `list()` — `skills` sans `body`. */
export interface ConfigListItemByDomain {
  providers: Provider;
  profiles: ModelProfile;
  mcp: McpServer;
  toolsets: Toolset;
  agents: Agent;
  skills: SkillMeta;
}

/** Forme d'un item de `get()`/`create()`/`update()` — `skills` avec `body`. */
export interface ConfigItemByDomain extends ConfigListItemByDomain {
  skills: SkillDetail;
}

export type ConfigListItem<D extends ConfigDomain = ConfigDomain> = ConfigListItemByDomain[D];
export type ConfigItem<D extends ConfigDomain = ConfigDomain> = ConfigItemByDomain[D];

export interface ConfigRepo {
  list<D extends ConfigDomain>(domain: D): Promise<ConfigListItem<D>[]>;
  get<D extends ConfigDomain>(domain: D, name: string): Promise<ConfigItem<D>>;
  create<D extends ConfigDomain>(domain: D, item: ConfigItem<D>): Promise<ConfigItem<D>>;
  update<D extends ConfigDomain>(
    domain: D, name: string, patch: Partial<ConfigItem<D>>,
  ): Promise<ConfigItem<D>>;
  remove(domain: ConfigDomain, name: string): Promise<void>;
  /** `is_default` provider = concept web-only. Impl RPC (F4) → rejet « non supporté ». */
  setDefaultProvider(name: string): Promise<void>;
  testProvider(name: string): Promise<{ models: string[] }>;
  testMcpServer(name: string): Promise<{ tools: string[] }>;
  listLocalTools(): Promise<string[]>;
}

// Ré-export pour les écrans (tâches 07-09), qui importent depuis '@vanyline/ui'.
export type {
  ConfigEntrySource,
  Provider, ProviderType, ModelProfile, McpServer, McpTransport, McpSelection,
  Toolset, Agent, AgentMode, SkillSelection, SkillMeta, SkillDetail,
} from '@vanyline/protocol';
export type { ChatTransport, UIMessage } from 'ai';
