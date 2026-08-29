import { onScopeDispose, ref, shallowRef, type Ref } from 'vue';
import type { SandboxStateEvent } from '../api/sandboxStateWs';
import { openSandboxStateWs } from '../api/sandboxStateWs';

/** Record réactif : nom sandbox → phase. Mis à jour par le hub WS en temps réel. */
export const sandboxPhases = ref(new Map<string, string>());

/** État global du hub WS (singleton). Un seul WebSocket par application. */
const ws = shallowRef<WebSocket | null>(null);
export const connected = ref(false);

let hubRunning = false;
let hubRef = 0;

/** Fonctions de rafraîchissement CRUD enregistrées par les dashboards. */
const refreshFns = new Set<() => void>();

export function registerRefresh(fn: () => void) {
  refreshFns.add(fn);
}
export function unregisterRefresh(fn: () => void) {
  refreshFns.delete(fn);
}

// Debounce : une salve d'événements (ex. replay initial à la reconnexion) ne
// déclenche qu'un seul rafraîchissement de l'interface.
let refreshScheduled = false;
function scheduleRefresh() {
  if (refreshScheduled) return;
  refreshScheduled = true;
  setTimeout(() => {
    refreshScheduled = false;
    for (const fn of refreshFns) fn();
  }, 300);
}

// Back-off de reconnexion : croît à chaque échec (ou fermeture immédiate —
// ex. utilisateur sans owner K8s, que le backend ferme aussitôt), remis à zéro
// après une connexion qui a tenu. Évite la boucle serrée de reconnexion.
const RECONNECT_MIN_MS = 3000;
const RECONNECT_MAX_MS = 30000;
const SESSION_STABLE_MS = 5000;
let reconnectDelay = RECONNECT_MIN_MS;

function bumpBackoff() {
  reconnectDelay = Math.min(reconnectDelay * 2, RECONNECT_MAX_MS);
}

function hubTick() {
  if (hubRunning) return;
  hubRunning = true;
  (async () => {
    while (hubRunning) {
      const startedAt = Date.now();
      try {
        const s = await openSandboxStateWs();
        ws.value = s;
        connected.value = true;

        let closed = false;
        const onClose = () => {
          closed = true;
          if (ws.value === s) {
            ws.value = null;
            connected.value = false;
          }
        };
        s.addEventListener('close', onClose, { once: true });
        s.addEventListener('error', () => {
          connected.value = false;
        });

        const evHandler = (ev: MessageEvent) => {
          if (closed) return;
          let event: SandboxStateEvent;
          try {
            event = JSON.parse(ev.data as string) as SandboxStateEvent;
          } catch {
            return;
          }
          if (!event.sandbox) return;
          const next = new Map(sandboxPhases.value);
          if (event.phase == null) {
            next.delete(event.sandbox);
          } else {
            next.set(event.sandbox, event.phase);
          }
          sandboxPhases.value = next;
          // Rafraîchir le store CRUD pour les champs hors WS (metadata, spec).
          scheduleRefresh();
        };
        s.addEventListener('message', evHandler);

        // Attendre la fin WS.
        await new Promise<void>((resolve) => {
          s.addEventListener('close', () => resolve(), { once: true });
        });

        // Session qui a tenu → on repart de zéro ; sinon (close quasi immédiat,
        // typiquement pas d'owner K8s côté backend) on temporise avant de retenter.
        if (Date.now() - startedAt >= SESSION_STABLE_MS) {
          reconnectDelay = RECONNECT_MIN_MS;
        } else {
          bumpBackoff();
          if (hubRunning) await new Promise((r) => setTimeout(r, reconnectDelay));
        }
      } catch {
        bumpBackoff();
        if (hubRunning) {
          await new Promise((r) => setTimeout(r, reconnectDelay));
        }
      }
    }
  })();
}

export function useSandboxState(): {
  phases: Ref<Map<string, string>>;
  connected: Ref<boolean>;
} {
  if (hubRef++ === 0) {
    hubTick();
  }
  onScopeDispose(() => {
    if (--hubRef === 0) {
      hubRunning = false;
      ws.value?.close();
      ws.value = null;
      connected.value = false;
    }
  });
  return {
    phases: sandboxPhases as Ref<Map<string, string>>,
    connected,
  };
}
