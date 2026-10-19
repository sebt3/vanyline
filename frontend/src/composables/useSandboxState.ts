import { onScopeDispose, ref, shallowRef, type Ref } from 'vue';
import type { SandboxStateEvent } from '../api/sandboxStateWs';
import { openSandboxStateWs } from '../api/sandboxStateWs';

/** Record réactif : nom sandbox → phase. Mis à jour par le hub WS en temps réel. */
export const sandboxPhases = ref(new Map<string, string>());
/** Record réactif : nom projet → phase (`Cloning` / `Cloned`). */
export const projectPhases = ref(new Map<string, string>());

/** État global du hub WS (singleton). Un seul WebSocket par application. */
const ws = shallowRef<WebSocket | null>(null);
export const connected = ref(false);

let hubRunning = false;
let hubRef = 0;

/** Fonctions de rafraîchissement CRUD enregistrées par les dashboards (événements `sandbox`). */
const refreshFns = new Set<() => void>();
/** Fonctions de rafraîchissement enregistrées pour les événements `project`. */
const projectRefreshFns = new Set<() => void>();

export function registerRefresh(fn: () => void) {
  refreshFns.add(fn);
}
export function unregisterRefresh(fn: () => void) {
  refreshFns.delete(fn);
}
export function registerProjectRefresh(fn: () => void) {
  projectRefreshFns.add(fn);
}
export function unregisterProjectRefresh(fn: () => void) {
  projectRefreshFns.delete(fn);
}

/** Debounce « schedule-once » par famille : une salve d'événements (ex. replay
 *  initial à la reconnexion) ne déclenche qu'un seul appel de chaque fonction. */
function makeDebouncedRun(run: () => void): () => void {
  let scheduled = false;
  return () => {
    if (scheduled) return;
    scheduled = true;
    setTimeout(() => {
      scheduled = false;
      run();
    }, 300);
  };
}
const scheduleRefresh = makeDebouncedRun(() => {
  for (const fn of refreshFns) fn();
});
const scheduleProjectRefresh = makeDebouncedRun(() => {
  for (const fn of projectRefreshFns) fn();
});

/** Met à jour une Map réactive : `phase` null retire l'entrée. */
function applyPhase(target: { value: Map<string, string> }, name: string, phase: string | null) {
  const next = new Map(target.value);
  if (phase == null) {
    next.delete(name);
  } else {
    next.set(name, phase);
  }
  target.value = next;
}

// Back-off de reconnexion : croît à chaque échec (ou fermeture immédiate —
// ex. utilisateur sans owner K8s, que le backend ferme aussitôt), remis à zéro
// après une connexion qui a tenu. Évite la boucle serrée de reconnexion.
const RECONNECT_MIN_MS = 3000;
const RECONNECT_MAX_MS = 30000;
const SESSION_STABLE_MS = 5000;
let reconnectDelay = RECONNECT_MIN_MS;

/** Attente de reconnexion interruptible : `reconnectNow()` la court-circuite. */
let wakeUp: (() => void) | null = null;
function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => {
    const done = () => {
      clearTimeout(timer);
      wakeUp = null;
      resolve();
    };
    const timer = setTimeout(done, ms);
    wakeUp = done;
  });
}

function bumpBackoff() {
  reconnectDelay = Math.min(reconnectDelay * 2, RECONNECT_MAX_MS);
}

/** Rouvre le socket tout de suite (ex. après création d'un Project : le backend
 *  ferme aussitôt le socket d'un utilisateur sans Owner). Coupe l'attente en
 *  cours et remet le back-off à zéro. Sans effet si le hub est arrêté ou si un
 *  socket est déjà ouvert. */
export function reconnectNow() {
  if (!hubRunning) return;
  reconnectDelay = RECONNECT_MIN_MS;
  if (ws.value) return;
  wakeUp?.();
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
          if (typeof event?.name !== 'string' || event.name === '') return;
          if (event.kind === 'sandbox') {
            applyPhase(sandboxPhases, event.name, event.phase);
            // Rafraîchir le store CRUD pour les champs hors WS (metadata, spec).
            scheduleRefresh();
          } else if (event.kind === 'project') {
            applyPhase(projectPhases, event.name, event.phase);
            scheduleProjectRefresh();
          }
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
          if (hubRunning) await sleep(reconnectDelay);
        }
      } catch {
        bumpBackoff();
        if (hubRunning) {
          await sleep(reconnectDelay);
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
