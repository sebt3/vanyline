import { ref, type Ref } from 'vue';
import { ApiError, createApiClient } from '../api/client';

export interface IdeActions {
  saveActiveFile?: () => void;
  closeActiveTab?: () => void;
  openExplorer?: () => void;
  newTerminal?: () => void;
  findInActiveFile?: () => void;
  replaceInActiveFile?: () => void;
}

import type { PagedResult } from './useCrudResource';

interface AgentOut {
  name: string;
}

interface ConversationOut {
  id: number;
}

/** Singleton partagé — pont entre le menu global (`MenuBar.vue`, monté une
 *  fois dans `App.vue`) et l'`IdeShell` actuellement ouvert pour une
 *  sandbox : `IdeShell`/`Editor` y enregistrent leurs handlers au montage
 *  (fusion, pas remplacement — plusieurs composants contribuent), le menu
 *  les invoque sans connaître ces composants. `activeConversationId` fait
 *  aussi office de signal pour la colonne assistant : elle ne doit exister
 *  que si une vraie session (conversation créée côté backend) est active. */
const ideActions = ref<IdeActions>({});
const activeConversationId: Ref<string | null> = ref(null);
const startingSession = ref(false);
const sessionError = ref<string | null>(null);

export function useIdeSession() {
  return { ideActions, activeConversationId, startingSession, sessionError };
}

/** Fusionne — n'écrase pas les handlers déjà enregistrés par un autre
 *  composant (ex. `Editor.vue` pour `saveActiveFile`, `IdeShell.vue` pour
 *  `closeActiveTab`). */
export function registerIdeActions(actions: IdeActions): void {
  ideActions.value = { ...ideActions.value, ...actions };
}

/** À appeler au démontage d'`IdeShell` (quitter la route sandbox) : plus de
 *  handlers valides, plus de session active à afficher. */
export function clearIdeActions(): void {
  ideActions.value = {};
  activeConversationId.value = null;
  sessionError.value = null;
}

/** Démarre une session agent : résout le premier agent configuré et crée
 *  la conversation associée, dans le contexte de la sandbox `sandboxName`
 *  (`POST /api/conversations` exige un `context` depuis
 *  docs/features/chat-app-fonctionnel.md, axe 1 — c'est lui qui permet au
 *  backend de résoudre les tools MCP de cette sandbox pour le tour). Pas de
 *  sélecteur d'agent pour ce MVP — un seul agent existe dans l'usage
 *  courant ; à revoir si plusieurs agents coexistent en pratique (choix
 *  arbitraire du premier, documenté ici plutôt que caché). */
export async function startAgentSession(sandboxName: string): Promise<void> {
  sessionError.value = null;
  startingSession.value = true;
  try {
    const client = createApiClient();
    const agentsPage = await client.get<PagedResult<AgentOut>>('/api/v1/agents');
    const agents = agentsPage.items;
    if (agents.length === 0) {
      sessionError.value = 'Aucun agent configuré — configure un agent dans Paramètres.';
      return;
    }
    const conv = await client.post<ConversationOut>('/api/conversations', {
      agent_name: agents[0].name,
      context: { kind: 'sandbox', data: { sandbox_name: sandboxName } },
    });
    activeConversationId.value = String(conv.id);
  } catch (e) {
    sessionError.value = e instanceof ApiError ? e.message : String(e);
  } finally {
    startingSession.value = false;
  }
}

/** Termine la session côté UI (ferme la colonne assistant) sans supprimer
 *  la conversation côté backend — l'historique reste accessible via
 *  `GET /api/conversations`. */
export function endAgentSession(): void {
  activeConversationId.value = null;
}
