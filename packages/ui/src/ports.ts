import type { ChatTransport, UIMessage } from 'ai';

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

export type { ChatTransport, UIMessage };

export type ConfigDomain = 'providers' | 'profiles' | 'mcp' | 'toolsets' | 'agents' | 'skills';

/** Entité de configuration name-keyed. Forme générique (le port est agnostique
 *  du backend) : `name` est la clé ; le reste sont les champs du domaine
 *  (`provider_type`, `endpoint`, `model`, `server_type`, `url`, ...). L'id
 *  interne (PK i32 côté app) n'est pas une clé du port — la résolution
 *  name↔id vit dans l'impl, jamais exposée ici. */
export interface ConfigItem {
  name: string;
  [key: string]: unknown;
}

export interface ConfigRepo {
  list(domain: ConfigDomain): Promise<ConfigItem[]>;
  get(domain: ConfigDomain, name: string): Promise<ConfigItem>;
  create(domain: ConfigDomain, item: ConfigItem): Promise<ConfigItem>;
  update(domain: ConfigDomain, name: string, patch: Partial<ConfigItem>): Promise<ConfigItem>;
  remove(domain: ConfigDomain, name: string): Promise<void>;
  testProvider(name: string): Promise<{ models: string[] }>;
  testMcpServer(name: string): Promise<{ tools: string[] }>;
  listLocalTools(): Promise<string[]>;
}