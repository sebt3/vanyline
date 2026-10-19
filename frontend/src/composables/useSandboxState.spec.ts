import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

// Faux WebSocket : le hub n'utilise que addEventListener / close, et déclenche
// 'message' / 'close' à partir des événements poussés par les tests.
class FakeSocket extends EventTarget {
  closed = false;
  close() {
    if (this.closed) return;
    this.closed = true;
    this.dispatchEvent(new Event('close'));
  }
  emit(data: unknown) {
    this.dispatchEvent(new MessageEvent('message', { data: JSON.stringify(data) }));
  }
}

const { openSandboxStateWs, sockets } = vi.hoisted(() => ({
  openSandboxStateWs: vi.fn(),
  sockets: [] as Array<{ closed: boolean; close(): void; emit(data: unknown): void }>,
}));

vi.mock('../api/sandboxStateWs', () => ({ openSandboxStateWs }));

type Hub = typeof import('./useSandboxState');

/** Laisse la boucle du hub consommer ses promesses en attente. */
async function flush() {
  await vi.advanceTimersByTimeAsync(0);
}

/** Module frais à chaque test : l'état du hub est un singleton de module. */
async function startHub(): Promise<{ hub: Hub; scope: { stop(): void } }> {
  const vue = await import('vue');
  const hub = await import('./useSandboxState');
  const scope = vue.effectScope();
  scope.run(() => hub.useSandboxState());
  await flush();
  return { hub, scope };
}

describe('useSandboxState (hub WebSocket)', () => {
  beforeEach(async () => {
    vi.useFakeTimers();
    vi.resetModules();
    sockets.length = 0;
    openSandboxStateWs.mockReset();
    openSandboxStateWs.mockImplementation(async () => {
      const s = new FakeSocket();
      sockets.push(s);
      return s;
    });
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it('evenement_project_declenche_le_rafraichissement_projet', async () => {
    const { hub, scope } = await startHub();
    const projectFn = vi.fn();
    const sandboxFn = vi.fn();
    hub.registerProjectRefresh(projectFn);
    hub.registerRefresh(sandboxFn);

    sockets[0]!.emit({ kind: 'project', name: 'vynil', phase: 'Cloned' });

    expect(hub.projectPhases.value.get('vynil')).toBe('Cloned');
    expect(projectFn).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(300);
    expect(projectFn).toHaveBeenCalledTimes(1);
    expect(sandboxFn).not.toHaveBeenCalled();
    scope.stop();
  });

  it('evenement_sandbox_ne_declenche_pas_le_rafraichissement_projet', async () => {
    const { hub, scope } = await startHub();
    const projectFn = vi.fn();
    const sandboxFn = vi.fn();
    hub.registerProjectRefresh(projectFn);
    hub.registerRefresh(sandboxFn);

    sockets[0]!.emit({ kind: 'sandbox', name: 'main', phase: 'Running' });

    expect(hub.sandboxPhases.value.get('main')).toBe('Running');
    await vi.advanceTimersByTimeAsync(300);
    expect(sandboxFn).toHaveBeenCalledTimes(1);
    expect(projectFn).not.toHaveBeenCalled();
    scope.stop();
  });

  it('kind_inconnu_ignore', async () => {
    const { hub, scope } = await startHub();
    const projectFn = vi.fn();
    const sandboxFn = vi.fn();
    hub.registerProjectRefresh(projectFn);
    hub.registerRefresh(sandboxFn);

    sockets[0]!.emit({ kind: 'autre', name: 'x', phase: 'y' });

    await vi.advanceTimersByTimeAsync(300);
    expect(hub.sandboxPhases.value.size).toBe(0);
    expect(hub.projectPhases.value.size).toBe(0);
    expect(projectFn).not.toHaveBeenCalled();
    expect(sandboxFn).not.toHaveBeenCalled();
    scope.stop();
  });

  it('phase null retire l\'entree de la Map (sandbox et projet)', async () => {
    const { hub, scope } = await startHub();
    sockets[0]!.emit({ kind: 'sandbox', name: 'main', phase: 'Running' });
    sockets[0]!.emit({ kind: 'project', name: 'vynil', phase: 'Cloning' });
    sockets[0]!.emit({ kind: 'sandbox', name: 'main', phase: null });
    sockets[0]!.emit({ kind: 'project', name: 'vynil', phase: null });

    expect(hub.sandboxPhases.value.has('main')).toBe(false);
    expect(hub.projectPhases.value.has('vynil')).toBe(false);
    scope.stop();
  });

  it('reconnectNow_rouvre_sans_attendre_le_backoff', async () => {
    const { hub, scope } = await startHub();
    expect(sockets).toHaveLength(1);

    // Fermeture immédiate : le hub temporise (3 s doublés à 6 s) avant de retenter.
    sockets[0]!.close();
    await flush();
    expect(sockets).toHaveLength(1);

    hub.reconnectNow();
    await flush();
    expect(sockets).toHaveLength(2);

    // Le délai est reparti à 3 s : une nouvelle fermeture immédiate donne 6 s, pas 12 s.
    sockets[1]!.close();
    await flush();
    await vi.advanceTimersByTimeAsync(5999);
    expect(sockets).toHaveLength(2);
    await vi.advanceTimersByTimeAsync(1);
    expect(sockets).toHaveLength(3);
    scope.stop();
  });

  it('une salve d\'evenements ne produit qu\'un rafraichissement par famille', async () => {
    const { hub, scope } = await startHub();
    const projectFn = vi.fn();
    hub.registerProjectRefresh(projectFn);

    sockets[0]!.emit({ kind: 'project', name: 'a', phase: 'Cloning' });
    sockets[0]!.emit({ kind: 'project', name: 'a', phase: 'Cloned' });
    sockets[0]!.emit({ kind: 'project', name: 'b', phase: 'Cloned' });
    await vi.advanceTimersByTimeAsync(300);

    expect(projectFn).toHaveBeenCalledTimes(1);
    scope.stop();
  });
});
