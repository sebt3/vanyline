import type { LogChannel, ServerHandle } from './rpc';

export type SupervisorStatus = 'starting' | 'ready' | 'error' | { restarting: number };

/** Item de status bar structurel (le vrai statusBarItem de l'éditeur satisfait ça). */
export interface StatusBarText {
  setText(text: string): void;
}

export interface BackoffConfig {
  baseMs: number; // 1000
  factor: number; // 2
  maxMs: number; // 30000
  maxRetries: number; // 5 — échecs consécutifs avant l'état 'erreur'
  stabilityMs: number; // 60000 — vivant aussi longtemps après ready → reset du compteur
}

export const DEFAULT_BACKOFF: BackoffConfig = {
  baseMs: 1000,
  factor: 2,
  maxMs: 30_000,
  maxRetries: 5,
  stabilityMs: 60_000,
};

export interface SupervisorDeps {
  channel: LogChannel;
  statusBar: StatusBarText;
  /** (Re)lance un serveur : doit rejeter ServerError en cas d'échec (wrap de startServer,
   *  qui relit la config à CHAQUE appel — le serveur peut redémarrer avec un autre binaire
   *  si vanyline.serverPath a changé entre-temps). */
  start: () => Promise<ServerHandle>;
  /** Affichage user-facing (showErrorMessage de l'éditeur côté extension.ts). */
  notifyError: (message: string) => void;
}

export interface Supervisor {
  /** Premier lancement (activate). Ne rejette JAMAIS : l'échec passe par
   *  statusBar 'erreur' + notifyError (activation dégradée, design « offline »). */
  start(): Promise<void>;
  /** Redémarrage manuel (vanyline.restartServer) : réinitialise le compteur de backoff
   *  et repart même depuis l'état 'erreur'. Ne rejette jamais non plus. */
  restart(): Promise<void>;
  /** Arrêt volontaire (deactivate) : shutdown/dispose du handle courant et AUCUN
   *  auto-redémarrage sur l'exit qui suit. */
  stop(): Promise<void>;
  /** Exposé pour les tests et la tâche 04 (relais d'état vers la webview). */
  current(): ServerHandle | undefined;
  onStatus(cb: (s: SupervisorStatus) => void): void;
}

const STATUS_TEXT: Record<'starting' | 'ready' | 'error', string> = {
  starting: 'vanyline: démarrage',
  ready: 'vanyline: prêt',
  error: 'vanyline: erreur',
};

/**
 * Machine à états pure du cycle de vie du serveur local : zéro dépendance à
 * l'éditeur, zéro spawn — tout passe par `SupervisorDeps` injecté. Les timers
 * sont ceux de l'environnement (vitest les fake, la production les vrais).
 */
export function createSupervisor(
  deps: SupervisorDeps,
  backoff: BackoffConfig = DEFAULT_BACKOFF,
): Supervisor {
  const callbacks: Array<(s: SupervisorStatus) => void> = [];

  let handle: ServerHandle | undefined;
  let stopped = false; // arrêt volontaire : stop(), ou extinction en cours pendant restart()
  let generation = 0; // invalide timers et await en vol des cycles précédents
  let failures = 0; // échecs consécutifs (crash post-ready, rejet, ou handle de retry mort)
  let retryTimer: ReturnType<typeof setTimeout> | undefined;
  let stabilityTimer: ReturnType<typeof setTimeout> | undefined;

  const setStatus = (s: SupervisorStatus): void => {
    const text =
      typeof s === 'string' ? STATUS_TEXT[s] : `vanyline: redémarrage (${s.restarting})`;
    deps.statusBar.setText(text);
    for (const cb of [...callbacks]) {
      cb(s);
    }
  };

  /** Point 7 : ce que l'utilisateur voit est aussi écrit dans le journal. */
  const reportError = (message: string): void => {
    deps.channel.appendLine(message);
    deps.notifyError(message);
  };

  const clearTimers = (): void => {
    if (retryTimer !== undefined) {
      clearTimeout(retryTimer);
      retryTimer = undefined;
    }
    if (stabilityTimer !== undefined) {
      clearTimeout(stabilityTimer);
      stabilityTimer = undefined;
    }
  };

  const delayFor = (attempt: number): number =>
    Math.min(backoff.baseMs * Math.pow(backoff.factor, attempt - 1), backoff.maxMs);

  const safeDispose = async (h: ServerHandle): Promise<void> => {
    try {
      await h.dispose();
    } catch {
      /* déjà mort : rien à faire */
    }
  };

  function giveUp(): void {
    setStatus('error');
    reportError(
      `Le serveur vanyline ne redémarre plus après ${backoff.maxRetries} tentatives consécutives. ` +
        "Vérifiez le paramètre vanyline.serverPath puis lancez la commande « vanyline.restartServer ».",
    );
  }

  /**
   * Événement d'échec — crash du serveur en place, rejet de deps.start(), ou handle
   * obtenu par une tentative déjà mort : on numérote la tentative suivante (1-based),
   * on affiche, on attend le délai, on relance. Après maxRetries échecs consécutifs,
   * on ne martèle pas : état 'erreur' et plus aucun retry programmé.
   * Un seul slot `retryTimer` : jamais deux timers de retry en parallèle.
   */
  function handleFailureEvent(gen: number): void {
    failures += 1;
    if (failures > backoff.maxRetries) {
      giveUp();
      return;
    }
    setStatus({ restarting: failures });
    retryTimer = setTimeout(() => {
      retryTimer = undefined;
      if (stopped || gen !== generation) return;
      void retryAttempt(gen);
    }, delayFor(failures));
  }

  /**
   * Adopte un handle résolu par deps.start() : écoute exit/error (les « error »
   * post-ready relèvent du superviseur depuis rpc.ts) + fenêtre de stabilité.
   * Un exit survenu entre la résolution et l'attachement du listener est rattrapé
   * par le test exitCode/signalCode : il enchaîne sur le délai suivant, jamais sur
   * un retry immédiat.
   */
  function adopt(h: ServerHandle, gen: number): void {
    handle = h;
    let down = false;
    const onDown = (): void => {
      if (down) return;
      down = true;
      if (stabilityTimer !== undefined) {
        clearTimeout(stabilityTimer);
        stabilityTimer = undefined;
      }
      if (stopped || gen !== generation) return;
      handle = undefined;
      handleFailureEvent(gen);
    };
    h.child.once('exit', onDown);
    h.child.once('error', onDown);
    if (h.child.exitCode !== null || h.child.signalCode !== null) {
      onDown();
      return;
    }
    setStatus('ready');
    stabilityTimer = setTimeout(() => {
      stabilityTimer = undefined;
      if (stopped || gen !== generation || handle !== h) return;
      failures = 0; // point 4 : vivant stabilityMs après ready → le compteur repart de zéro
    }, backoff.stabilityMs);
  }

  /** Premier lancement et redémarrage manuel : l'échec de CET appel-là ne relance rien. */
  async function launch(gen: number): Promise<void> {
    setStatus('starting');
    try {
      const h = await deps.start();
      if (stopped || gen !== generation) {
        await safeDispose(h); // handle obtenu trop tard (stop/restart pendant l'await)
        return;
      }
      adopt(h, gen);
    } catch (err) {
      if (stopped || gen !== generation) return;
      setStatus('error');
      reportError(err instanceof Error ? err.message : String(err));
    }
  }

  /** Tentative de retry : l'échec enchaîne sur le niveau de backoff suivant. */
  async function retryAttempt(gen: number): Promise<void> {
    try {
      const h = await deps.start();
      if (stopped || gen !== generation) {
        await safeDispose(h);
        return;
      }
      adopt(h, gen);
    } catch {
      if (stopped || gen !== generation) return;
      handleFailureEvent(gen);
    }
  }

  return {
    async start(): Promise<void> {
      const gen = ++generation;
      stopped = false;
      clearTimers();
      failures = 0;
      await launch(gen);
    },

    async restart(): Promise<void> {
      const gen = ++generation;
      clearTimers();
      failures = 0; // point 5 : le débouché manuel repart de zéro, même depuis 'erreur'
      const current = handle;
      handle = undefined;
      if (current) {
        stopped = true; // éteint le handle vivant sans passer par le chemin crash
        await safeDispose(current);
        if (gen !== generation) return; // stop()/restart() appelé pendant l'extinction
        stopped = false;
      }
      await launch(gen);
    },

    async stop(): Promise<void> {
      stopped = true; // garde « intentional stop » posée AVANT le dispose
      generation += 1;
      clearTimers();
      const current = handle;
      handle = undefined;
      if (current) {
        await safeDispose(current);
      }
    },

    current(): ServerHandle | undefined {
      return handle;
    },

    onStatus(cb: (s: SupervisorStatus) => void): void {
      callbacks.push(cb);
    },
  };
}
