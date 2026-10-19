import type { ChatTransport, UIMessage, UIMessageChunk } from 'ai';
import type { ChatBackend, ChatConversation, ChatMessageRecord } from '@vanyline/ui';
import { ApiError, createApiClient } from './client';
import type { PagedResult } from '../composables/useCrudResource';

/** Types de contexte de chat (cf. `components/ContextChat.sdd`, `app/src/api/conversations.sdd`). */
export type ContextKind = 'settings' | 'home' | 'project' | 'sandbox';
export type ContextData = Record<string, unknown>;

/** Clé stable de l'agent système par contexte (`app/src/system_agents.sdd`). */
const SYSTEM_KEYS: Record<ContextKind, string> = {
  settings: 'config',
  home: 'home',
  project: 'project',
  sandbox: 'sandbox',
};

/** Nom de la ligne d'agent système du contexte : `system-<clé>`. */
export function systemAgentName(kind: ContextKind): string {
  return `system-${SYSTEM_KEYS[kind]}`;
}

/** Marqueur d'erreur « aucun modèle configuré » (422, `lib/src/error.rs`). */
export const NO_MODEL_CONFIGURED_CODE = 'VNL-LLM-008';

export interface ConversationContextOut {
  id: number;
  kind: string;
  data: unknown;
  created_at: string;
}

/** Forme de `ConversationOut` (`app/src/api/conversations.rs`). */
export interface ConversationOut {
  id: number;
  agent_name: string | null;
  context: ConversationContextOut;
  title: string | null;
  todo: string | null;
  created_at: string;
  updated_at: string;
}

/** Ligne d'agent telle que renvoyée par `/api/v1/agents`. Toute la ligne est conservée
 *  (un PUT renvoie la forme complète, les gardes des agents système la comparent). */
export type AgentRow = { id: number; name: string; system_key: string | null; model_profile_id: number } & Record<string, unknown>;

/** Profil de modèle (`/api/v1/model-profiles`) — seuls `id` et `name` sont lus ici. */
export type ModelProfileRow = { id: number; name: string } & Record<string, unknown>;

interface MessageRow {
  id: number;
  role: string;
  payload: { content?: string };
  created_at: string;
}

const api = createApiClient();

/** Vrai pour le 422 « aucun profil de modèle » (et seulement lui). */
export function isNoModelProfileError(err: unknown): boolean {
  return err instanceof ApiError && err.status === 422 && err.code === NO_MODEL_CONFIGURED_CODE;
}

/** Vrai si le texte d'une erreur de tour porte le marqueur « aucun modèle ». */
export function isNoModelProfileMessage(text: string): boolean {
  return text.includes(NO_MODEL_CONFIGURED_CODE);
}

/** Comparaison structurelle insensible à l'ordre des clés. */
function sameData(a: unknown, b: ContextData): boolean {
  return JSON.stringify(sortKeys(a)) === JSON.stringify(sortKeys(b));
}

function sortKeys(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(sortKeys);
  if (value && typeof value === 'object') {
    const out: Record<string, unknown> = {};
    for (const key of Object.keys(value).sort()) {
      out[key] = sortKeys((value as Record<string, unknown>)[key]);
    }
    return out;
  }
  return value;
}

function matchesContext(c: ConversationOut, kind: ContextKind, data: ContextData): boolean {
  return c.context.kind === kind && sameData(c.context.data, data);
}

/** Toutes les conversations de l'utilisateur, plus récentes d'abord (ordre du serveur). */
export async function listConversations(): Promise<ConversationOut[]> {
  return api.get<ConversationOut[]>('/api/conversations');
}

/** Conversation la plus récente du contexte exact (kind + data), sinon `null`. */
export async function findContextConversation(
  kind: ContextKind,
  data: ContextData,
): Promise<ConversationOut | null> {
  const rows = await listConversations();
  return rows.find((c) => matchesContext(c, kind, data)) ?? null;
}

/** Crée une conversation dans le contexte avec l'agent donné. */
export async function createContextConversation(
  kind: ContextKind,
  data: ContextData,
  agentName: string,
): Promise<ConversationOut> {
  return api.post<ConversationOut>('/api/conversations', {
    agent_name: agentName,
    context: { kind, data },
  });
}

/** Réutilise la conversation du contexte, sinon en crée une avec l'agent par défaut. */
export async function findOrCreateContextConversation(
  kind: ContextKind,
  data: ContextData,
  defaultAgent: string,
): Promise<ConversationOut> {
  const existing = await findContextConversation(kind, data);
  return existing ?? createContextConversation(kind, data, defaultAgent);
}

async function listAll<T>(base: string): Promise<T[]> {
  const first = await api.get<T[] | PagedResult<T>>(base);
  if (Array.isArray(first)) return first;
  const rows = [...first.items];
  for (let page = first.page + 1; page <= first.total_pages; page += 1) {
    const next = await api.get<T[] | PagedResult<T>>(`${base}?page=${page}`);
    rows.push(...(Array.isArray(next) ? next : next.items));
  }
  return rows;
}

export async function listAgents(): Promise<AgentRow[]> {
  return listAll<AgentRow>('/api/v1/agents');
}

export async function listModelProfiles(): Promise<ModelProfileRow[]> {
  return listAll<ModelProfileRow>('/api/v1/model-profiles');
}

/** Change le modèle de l'agent de configuration. Seul `model_profile_id` diffère de la
 *  ligne lue : les autres champs (prompt, nom, clé système, version) sont renvoyés tels quels. */
export async function setConfigAgentModel(agent: AgentRow, modelProfileId: number): Promise<AgentRow> {
  if (agent.system_key !== SYSTEM_KEYS.settings) {  // 'config' : agent système du contexte settings
    throw new Error('seul le modèle de l’agent de configuration est modifiable ici');
  }
  return api.put<AgentRow>(`/api/v1/agents/${agent.id}`, {
    ...agent,
    model_profile_id: modelProfileId,
  });
}

/** Texte lisible d'une erreur (message du serveur déjà sans secret par contrat). */
export function contextChatErrorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

async function loadMessages(conversationId: string): Promise<ChatMessageRecord[]> {
  const rows = await api.get<MessageRow[]>(`/api/conversations/${conversationId}/messages`);
  return rows.map((m) => ({
    id: String(m.id),
    role: m.role === 'user' ? 'user' : 'assistant',
    content: m.payload.content ?? '',
  }));
}

/** Source du contexte, lue à chaque appel (les props peuvent changer). */
export interface ContextSource {
  kind(): ContextKind;
  data(): ContextData;
  agent(): string;
}

/** Backend de `ChatWindow` pour un contexte donné. `createConversation` utilise l'agent
 *  courant de la source. */
export function contextChatBackend(source: ContextSource): ChatBackend {
  return {
    async listConversations(): Promise<ChatConversation[]> {
      const rows = await listConversations();
      return rows
        .filter((c) => matchesContext(c, source.kind(), source.data()))
        .map((c) => ({ id: String(c.id), title: c.title, createdAt: c.created_at }));
    },
    loadMessages,
    async createConversation(): Promise<string> {
      const conv = await createContextConversation(source.kind(), source.data(), source.agent());
      return String(conv.id);
    },
  };
}

export interface TurnObserver {
  /** Appelé une seule fois à la fin de chaque tour (succès, erreur ou interruption). */
  onTurnFinished(): void;
  /** Appelé pour chaque chunk d'erreur de tour, avec son texte. */
  onTurnError(message: string): void;
}

/** Enveloppe un transport `ai` pour observer la fin et les erreurs de chaque tour. */
export function observeTurns(
  inner: ChatTransport<UIMessage>,
  observer: TurnObserver,
): ChatTransport<UIMessage> {
  return {
    async sendMessages(options) {
      const stream = await inner.sendMessages(options);
      return observeStream(stream, observer);
    },
    reconnectToStream(options) {
      return inner.reconnectToStream(options);
    },
  };
}

function observeStream(
  stream: ReadableStream<UIMessageChunk>,
  observer: TurnObserver,
): ReadableStream<UIMessageChunk> {
  const reader = stream.getReader();
  let finished = false;
  const finish = () => {
    if (finished) return;
    finished = true;
    observer.onTurnFinished();
  };

  return new ReadableStream<UIMessageChunk>({
    async pull(controller) {
      let result: ReadableStreamReadResult<UIMessageChunk>;
      try {
        result = await reader.read();
      } catch (err) {
        finish();
        controller.error(err);
        return;
      }
      if (result.done) {
        finish();
        controller.close();
        return;
      }
      const chunk = result.value;
      if (chunk.type === 'error') observer.onTurnError(chunk.errorText);
      controller.enqueue(chunk);
    },
    cancel(reason) {
      finish();
      return reader.cancel(reason);
    },
  });
}
