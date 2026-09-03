import { EventEmitter } from 'node:events';
import type { ChildProcess } from 'node:child_process';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Mock } from 'vitest';
import type { ServerHandle } from './rpc';
import {
  createSupervisor,
  type BackoffConfig,
  type Supervisor,
  type SupervisorDeps,
  type SupervisorStatus,
} from './supervisor';

/** Faux ChildProcess : EventEmitter + exitCode/kills (même esprit que rpc.spec.ts). */
class FakeChild extends EventEmitter {
  readonly kills: Array<string | number | undefined> = [];
  exitCode: number | null = null;
  signalCode: string | null = null;

  kill(signal?: string | number): boolean {
    this.kills.push(signal);
    return true;
  }

  emitExit(code: number): void {
    this.exitCode = code;
    this.signalCode = null;
    this.emit('exit', code, null);
  }

  asChildProcess(): ChildProcess {
    return this as unknown as ChildProcess;
  }
}

interface FakeHandle {
  handle: ServerHandle;
  child: FakeChild;
  dispose: Mock<() => Promise<void>>;
}

function makeHandle(): FakeHandle {
  const child = new FakeChild();
  const dispose = vi.fn(async (): Promise<void> => {});
  return {
    handle: { conn: undefined as never, child: child.asChildProcess(), dispose },
    child,
    dispose,
  };
}

/** Handle déjà mort à l'adoption : exit émis avant que le superviseur n'attache son listener. */
function deadHandle(): FakeHandle {
  const h = makeHandle();
  h.child.emitExit(1);
  return h;
}

/** Équivalent typé de Promise.withResolvers (lib TS du tsconfig extension = ES2022). */
interface Deferred<T> {
  promise: Promise<T>;
  resolve(value: T): void;
  reject(reason: unknown): void;
}

function deferred<T>(): Deferred<T> {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

interface Harness {
  sup: Supervisor;
  texts: string[];
  statuses: SupervisorStatus[];
  logged: string[];
  notified: string[];
  start: Mock<() => Promise<ServerHandle>>;
}

function setup(backoff?: BackoffConfig): Harness {
  const texts: string[] = [];
  const statuses: SupervisorStatus[] = [];
  const logged: string[] = [];
  const notified: string[] = [];
  const start = vi.fn<() => Promise<ServerHandle>>();

  const deps: SupervisorDeps = {
    channel: {
      appendLine: (v) => {
        logged.push(v);
      },
    },
    statusBar: {
      setText: (t) => {
        texts.push(t);
      },
    },
    start: () => start(),
    notifyError: (m) => {
      notified.push(m);
    },
  };

  const sup = createSupervisor(deps, backoff);
  sup.onStatus((s) => {
    statuses.push(s);
  });
  return { sup, texts, statuses, logged, notified, start };
}

/** Démarre et résout le premier start() avec un handle vivant. */
async function bootLive(h: Harness): Promise<FakeHandle> {
  const live = makeHandle();
  const d = deferred<ServerHandle>();
  h.start.mockImplementationOnce(() => d.promise);
  const pending = h.sup.start();
  d.resolve(live.handle);
  await pending;
  return live;
}

/** Vide les microtâches en attente sous timers faux (résolutions de deferred). */
const flush = () => vi.advanceTimersByTimeAsync(0);

beforeEach(() => {
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
});

describe('createSupervisor', () => {
  it('start happy : démarrage → prêt, aucun auto-retry', async () => {
    const h = setup();
    const live = makeHandle();
    const d = deferred<ServerHandle>();
    h.start.mockImplementation(() => d.promise);

    const pending = h.sup.start();
    d.resolve(live.handle);
    await pending;

    expect(h.texts).toEqual(['vanyline: démarrage', 'vanyline: prêt']);
    expect(h.statuses).toEqual(['starting', 'ready']);
    expect(h.sup.current()).toBe(live.handle);

    // « pas de timer en attente » côté retry : rien ne relance, même après 10 min.
    await vi.advanceTimersByTimeAsync(600_000);
    expect(h.start).toHaveBeenCalledTimes(1);
    expect(h.texts).toHaveLength(2);
  });

  it('start échoué : erreur + notifyError(message), aucun auto-retry du premier lancement', async () => {
    const h = setup();
    h.start.mockRejectedValueOnce(new Error('boom'));

    await h.sup.start(); // ne rejette jamais

    expect(h.texts).toEqual(['vanyline: démarrage', 'vanyline: erreur']);
    expect(h.statuses).toEqual(['starting', 'error']);
    expect(h.notified).toHaveLength(1);
    expect(h.notified[0]).toContain('boom');
    expect(h.logged).toEqual([expect.stringContaining('boom')]); // point 7 : journal
    expect(h.sup.current()).toBeUndefined();

    await vi.advanceTimersByTimeAsync(60_000);
    expect(h.start).toHaveBeenCalledTimes(1);
  });

  it('crash → backoff exponentiel 1s,2s,4s,8s,16s puis erreur après la 5e tentative', async () => {
    const h = setup();
    const live0 = await bootLive(h);

    // 5 tentatives de retry, chacune résolue avec un handle déjà mort (crash-loop offline).
    const attempts = Array.from({ length: 5 }, () => deferred<ServerHandle>());
    for (const a of attempts) {
      h.start.mockImplementationOnce(() => a.promise);
    }

    live0.child.emitExit(1);
    expect(h.texts.at(-1)).toBe('vanyline: redémarrage (1)');

    const delays = [1000, 2000, 4000, 8000, 16000];
    for (const [i, delay] of delays.entries()) {
      await vi.advanceTimersByTimeAsync(delay - 1);
      expect(h.start).toHaveBeenCalledTimes(i + 1); // pas rappelé avant l'échéance
      await vi.advanceTimersByTimeAsync(1);
      expect(h.start).toHaveBeenCalledTimes(i + 2); // tentative n°(i+1) lancée
      attempts[i].resolve(deadHandle().handle);
      await flush();
      if (i < delays.length - 1) {
        expect(h.texts.at(-1)).toBe(`vanyline: redémarrage (${i + 2})`);
      }
    }

    expect(h.texts.at(-1)).toBe('vanyline: erreur');
    expect(h.statuses.at(-1)).toBe('error');
    expect(h.notified).toHaveLength(1);
    expect(h.notified[0]).toContain('vanyline.restartServer');
    expect(h.logged.join('\n')).toContain('vanyline.restartServer');

    await vi.advanceTimersByTimeAsync(600_000); // 10 minutes
    expect(h.start).toHaveBeenCalledTimes(6); // 1 initial + 5 tentatives, plus jamais
  });

  it('délai plafonné : delays 1000, 2000, 3000, 3000 (maxMs), pas 4000', async () => {
    const h = setup({ baseMs: 1000, factor: 2, maxMs: 3000, maxRetries: 9, stabilityMs: 60000 });
    const live0 = await bootLive(h);

    const attempts = Array.from({ length: 4 }, () => deferred<ServerHandle>());
    for (const a of attempts) {
      h.start.mockImplementationOnce(() => a.promise);
    }

    live0.child.emitExit(1);
    expect(h.texts.at(-1)).toBe('vanyline: redémarrage (1)');

    const delays = [1000, 2000, 3000, 3000]; // min(1000·2^(n-1), 3000), plafonné dès n=3
    for (const [i, delay] of delays.entries()) {
      await vi.advanceTimersByTimeAsync(delay - 1);
      expect(h.start).toHaveBeenCalledTimes(i + 1);
      await vi.advanceTimersByTimeAsync(1);
      expect(h.start).toHaveBeenCalledTimes(i + 2);
      attempts[i].resolve(deadHandle().handle);
      await flush();
      expect(h.texts.at(-1)).toBe(`vanyline: redémarrage (${i + 2})`);
    }
    expect(h.notified).toHaveLength(0); // maxRetries=9 : pas encore donné
  });

  it('stabilité : 60 s vivant remet le compteur à 0 malgré une crash-loop antérieure', async () => {
    const h = setup();
    const handles = [makeHandle(), makeHandle(), makeHandle(), makeHandle()];
    const defs = handles.map(() => deferred<ServerHandle>());
    for (const d of defs) {
      h.start.mockImplementationOnce(() => d.promise);
    }

    const pending = h.sup.start();
    defs[0].resolve(handles[0].handle);
    await pending;
    expect(h.statuses).toEqual(['starting', 'ready']);

    // Deux crashes pour monter le compteur à 2 (chaque retry aboutit à un handle vivant).
    handles[0].child.emitExit(1);
    expect(h.texts.at(-1)).toBe('vanyline: redémarrage (1)');
    await vi.advanceTimersByTimeAsync(1000);
    expect(h.start).toHaveBeenCalledTimes(2);
    defs[1].resolve(handles[1].handle);
    await flush();
    expect(h.texts.at(-1)).toBe('vanyline: prêt');

    handles[1].child.emitExit(1);
    expect(h.texts.at(-1)).toBe('vanyline: redémarrage (2)'); // compteur conservé (échec du retry 1)
    await vi.advanceTimersByTimeAsync(2000);
    expect(h.start).toHaveBeenCalledTimes(3);
    defs[2].resolve(handles[2].handle);
    await flush();
    expect(h.texts.at(-1)).toBe('vanyline: prêt');

    // Phase stable : vivant 60 s après prêt → reset du compteur.
    await vi.advanceTimersByTimeAsync(60_000);

    handles[2].child.emitExit(1);
    expect(h.texts.at(-1)).toBe('vanyline: redémarrage (1)'); // remis à 0, pas (3)
    await vi.advanceTimersByTimeAsync(999);
    expect(h.start).toHaveBeenCalledTimes(3);
    await vi.advanceTimersByTimeAsync(1);
    expect(h.start).toHaveBeenCalledTimes(4); // et délai niveau 1 : 1000 ms
    defs[3].resolve(handles[3].handle);
    await flush();
    expect(h.texts.at(-1)).toBe('vanyline: prêt');
  });

  it('exit pendant un retry : enchaîne au délai du niveau 2, jamais immédiat', async () => {
    const h = setup();
    const live0 = await bootLive(h);

    const d1 = deferred<ServerHandle>();
    const d2 = deferred<ServerHandle>();
    h.start.mockImplementationOnce(() => d1.promise);
    h.start.mockImplementationOnce(() => d2.promise);

    live0.child.emitExit(1);
    expect(h.texts.at(-1)).toBe('vanyline: redémarrage (1)');
    await vi.advanceTimersByTimeAsync(1000);
    expect(h.start).toHaveBeenCalledTimes(2);

    // Le 2e start() résout avec un handle qui meurt dans le trou entre
    // résolution et attachement du listener (point 3).
    const dying = makeHandle();
    d1.resolve(dying.handle);
    dying.child.emitExit(1);
    await flush();

    expect(h.texts.at(-1)).toBe('vanyline: redémarrage (2)');
    await vi.advanceTimersByTimeAsync(1999);
    expect(h.start).toHaveBeenCalledTimes(2); // jamais deux timers en parallèle, pas de martelage
    await vi.advanceTimersByTimeAsync(1);
    expect(h.start).toHaveBeenCalledTimes(3);

    d2.resolve(makeHandle().handle);
    await flush();
    expect(h.texts.at(-1)).toBe('vanyline: prêt');
  });

  it('restart() depuis erreur : compteur remis, start immédiat, retour possible au prêt', async () => {
    const h = setup();
    const live0 = await bootLive(h);

    // On monte jusqu'à l'état erreur comme au cas 3 (5 tentatives mortes).
    const attempts = Array.from({ length: 5 }, () => deferred<ServerHandle>());
    for (const a of attempts) {
      h.start.mockImplementationOnce(() => a.promise);
    }
    live0.child.emitExit(1);
    for (const [i, delay] of [1000, 2000, 4000, 8000, 16000].entries()) {
      await vi.advanceTimersByTimeAsync(delay);
      attempts[i].resolve(deadHandle().handle);
      await flush();
    }
    expect(h.texts.at(-1)).toBe('vanyline: erreur');
    const callsBefore = h.start.mock.calls.length; // 6 = 1 initial + 5 tentatives

    const d = deferred<ServerHandle>();
    h.start.mockImplementationOnce(() => d.promise);
    const live = makeHandle();

    const pending = h.sup.restart();
    expect(h.start).toHaveBeenCalledTimes(callsBefore + 1); // immédiat, sans délai
    expect(h.texts.at(-1)).toBe('vanyline: démarrage');
    d.resolve(live.handle);
    await pending;

    expect(h.texts.at(-1)).toBe('vanyline: prêt');
    expect(h.statuses.at(-1)).toBe('ready');
    expect(h.sup.current()).toBe(live.handle);
  });

  it('restart() avec handle vivant : dispose() appelé, sans auto-retry pendant l extinction', async () => {
    const h = setup();
    const live0 = await bootLive(h);

    const d1 = deferred<ServerHandle>();
    h.start.mockImplementationOnce(() => d1.promise);

    const restarting = h.sup.restart();
    await flush(); // laisse l'extinction (dispose) puis le launch démarrer, sans délai
    expect(live0.dispose).toHaveBeenCalledTimes(1);
    expect(h.texts.at(-1)).toBe('vanyline: démarrage');
    expect(h.start).toHaveBeenCalledTimes(2); // immédiat, sans délai de backoff

    // L'exit « volontaire » de l'extinction ne doit pas déclencher le chemin crash.
    live0.child.emitExit(0);
    d1.resolve(makeHandle().handle);
    await restarting;

    expect(h.texts.at(-1)).toBe('vanyline: prêt');
    expect(h.start).toHaveBeenCalledTimes(2);
    expect(h.notified).toEqual([]);
  });

  it('stop() : dispose du handle courant, aucune relance sur l exit qui suit', async () => {
    const h = setup();
    const live0 = await bootLive(h);

    await h.sup.stop();
    expect(live0.dispose).toHaveBeenCalledTimes(1);
    expect(h.sup.current()).toBeUndefined();

    live0.child.emitExit(0);
    await flush();
    await vi.advanceTimersByTimeAsync(60_000);
    expect(h.start).toHaveBeenCalledTimes(1);
    expect(h.texts).toEqual(['vanyline: démarrage', 'vanyline: prêt']);
    expect(h.notified).toEqual([]);
  });

  it('stop() pendant un retry programmé : le timer n appelle plus start', async () => {
    const h = setup();
    const live0 = await bootLive(h);

    live0.child.emitExit(1);
    expect(h.texts.at(-1)).toBe('vanyline: redémarrage (1)');

    await h.sup.stop();
    await vi.advanceTimersByTimeAsync(60_000); // échéance du timer de retry dépassée
    expect(h.start).toHaveBeenCalledTimes(1);
    expect(h.texts.at(-1)).toBe('vanyline: redémarrage (1)'); // aucun nouveau statut
  });
});
