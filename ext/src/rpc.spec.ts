import { EventEmitter } from 'node:events';
import { PassThrough } from 'node:stream';
import type { ChildProcess, SpawnOptions } from 'node:child_process';
import { describe, expect, it } from 'vitest';
import {
  ServerError,
  createStdioTransport,
  startServer,
  type LogChannel,
} from './rpc';

/** Faux ChildProcess : PassThrough stdin/stdout/stderr + EventEmitter exit/kill. */
class FakeChild extends EventEmitter {
  readonly stdin = new PassThrough();
  readonly stdout = new PassThrough();
  readonly stderr = new PassThrough();
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

/** Fake child qui répond sur stdout aux trames lues sur stdin (répondeur ndjson). */
function responderChild(
  onRequest: (req: Record<string, unknown>) => Record<string, unknown> | undefined,
  afterResponse?: (req: Record<string, unknown>) => void,
): FakeChild {
  const child = new FakeChild();
  let buffer = '';
  child.stdin.on('data', (chunk: Buffer) => {
    buffer += chunk.toString('utf8');
    let idx = buffer.indexOf('\n');
    while (idx >= 0) {
      const line = buffer.slice(0, idx);
      buffer = buffer.slice(idx + 1);
      try {
        const req = JSON.parse(line) as Record<string, unknown>;
        const resp = onRequest(req);
        if (resp) {
          // Un vrai process enfant ne peut JAMAIS répondre de façon synchrone
          // dans le write() du client. Sans ce délai, la réponse arriverait
          // avant que RpcConnection.request ait enregistré sa pending
          // (write-then-register, packages/protocol/src/connection.ts:123)
          // → réponse jetée (id inconnu), request suspendue.
          setTimeout(() => {
            child.stdout.write(`${JSON.stringify({ jsonrpc: '2.0', id: req.id, ...resp })}\n`);
            afterResponse?.(req);
          }, 0);
        }
      } catch {
        // trame non parsible : ignorée (le protocole côté client ne doit pas en émettre)
      }
      idx = buffer.indexOf('\n');
    }
  });
  return child;
}

const okInitialize = (req: Record<string, unknown>): Record<string, unknown> | undefined =>
  req.method === 'initialize'
    ? { result: { protocolVersion: 1, serverVersion: 'test' } }
    : undefined;

const noopChannel: LogChannel = { appendLine: () => {} };

const tick = () => new Promise<void>((resolve) => setTimeout(resolve, 0));

/** Récupère la raison de rejet d'une promise (undefined si elle résout). */
async function rejection(promise: Promise<unknown>): Promise<unknown> {
  return promise.then(
    () => {
      throw new Error('la promise devait rejeter');
    },
    (err: unknown) => err,
  );
}

describe('createStdioTransport', () => {
  it("write() ajoute le '\\n' dans stdin", () => {
    const child = new FakeChild();
    const transport = createStdioTransport(child);
    transport.write('{"a":1}');
    expect(String(child.stdin.read())).toBe('{"a":1}\n');
  });

  it('reconstitue les lignes complètes depuis des fragments stdout', async () => {
    const child = new FakeChild();
    const transport = createStdioTransport(child);
    const lines: string[] = [];
    transport.onLine((line) => lines.push(line));

    child.stdout.write('{"id":1}\n{"id":2');
    await tick();
    expect(lines).toEqual(['{"id":1}']);

    child.stdout.write(',"x":2}\n');
    await tick();
    expect(lines).toEqual(['{"id":1}', '{"id":2,"x":2}']);
  });
});

describe('startServer', () => {
  it('happy path : argv figé, shell:false, cwd, RUST_LOG, initialize et résolution', async () => {
    const child = responderChild(okInitialize);
    const calls: Array<{ bin: string; args: string[]; options: SpawnOptions }> = [];

    const handle = await startServer({
      spawn: (bin, args, options) => {
        calls.push({ bin, args, options });
        return child.asChildProcess();
      },
      channel: noopChannel,
      bin: '/usr/local/bin/vanyline',
      workspace: '/home/dev/monprojet',
      logLevel: 'debug',
    });

    expect(handle.conn).toBeDefined();
    expect(handle.child).toBe(child.asChildProcess());

    expect(calls).toHaveLength(1);
    expect(calls[0].bin).toBe('/usr/local/bin/vanyline');
    expect(calls[0].args).toEqual(['serve', '--stdio']);
    expect(calls[0].options.shell).toBe(false);
    expect(calls[0].options.cwd).toBe('/home/dev/monprojet');
    expect(calls[0].options.env?.RUST_LOG).toBe('debug');
    expect(calls[0].options.env?.PATH).toBe(process.env.PATH);
  });

  it("la trame initialize émise porte params.protocolVersion = 1 et params.workspace", async () => {
    const requests: Array<Record<string, unknown>> = [];
    const child = responderChild((req) => {
      requests.push(req);
      return okInitialize(req);
    });

    await startServer({
      spawn: () => child.asChildProcess(),
      channel: noopChannel,
      bin: 'vanyline',
      workspace: '/work/dir',
    });

    const init = requests.find((r) => r.method === 'initialize');
    expect(init).toBeDefined();
    const params = init?.params as Record<string, unknown>;
    expect(params.protocolVersion).toBe(1);
    expect(params.workspace).toBe('/work/dir');
  });

  it('mismatch protocole → ServerError VNL-EXT-011 / VNL-RPC-003', async () => {
    const child = responderChild((req) =>
      req.method === 'initialize'
        ? {
            error: {
              code: -32000,
              message: 'Unknown protocol version: got 1, expected 2',
              data: { code: 'VNL-RPC-003' },
            },
          }
        : undefined,
    );

    const err = await rejection(
      startServer({
        spawn: () => child.asChildProcess(),
        channel: noopChannel,
        bin: 'vanyline',
      }),
    );

    expect(err).toBeInstanceOf(ServerError);
    expect((err as ServerError).vnlExtCode).toBe('VNL-EXT-011');
    expect((err as ServerError).serverCode).toBe('VNL-RPC-003');
    expect((err as Error).message).toMatch(/protocole/i);
    expect((err as Error).message).toMatch(/vanyline/);
  });

  it('enfant meurt pendant initialize → ServerError VNL-EXT-011 (pas de hang)', async () => {
    const child = new FakeChild();
    const pending = startServer({
      spawn: () => child.asChildProcess(),
      channel: noopChannel,
      bin: 'vanyline',
    });
    setTimeout(() => child.emitExit(127), 0);

    const err = await rejection(pending);
    expect(err).toBeInstanceOf(ServerError);
    expect((err as ServerError).vnlExtCode).toBe('VNL-EXT-011');
  });

  it("binaire introuvable ('error' ENOENT) → ServerError VNL-EXT-010", async () => {
    const child = new FakeChild();
    const pending = startServer({
      spawn: () => child.asChildProcess(),
      channel: noopChannel,
      bin: 'vanyline-missing',
    });
    setTimeout(() => {
      child.emit('error', Object.assign(new Error('spawn vanyline-missing ENOENT'), { code: 'ENOENT' }));
    }, 0);

    const err = await rejection(pending);
    expect(err).toBeInstanceOf(ServerError);
    expect((err as ServerError).vnlExtCode).toBe('VNL-EXT-010');
    expect((err as Error).message).toContain('ENOENT');
    expect((err as Error).message).toContain('vanyline-missing');
  });

  it('stderr multi-lignes → appendLine par ligne', async () => {
    const logged: string[] = [];
    const child = responderChild(okInitialize);

    await startServer({
      spawn: () => child.asChildProcess(),
      channel: { appendLine: (v) => logged.push(v) },
      bin: 'vanyline',
    });

    child.stderr.write('a\nb');
    await tick();
    child.stderr.write('c\n');
    await tick();

    expect(logged).toEqual(['a', 'b', 'c']);
  });

  it('dispose : réponse shutdown + exit → pas de kill', async () => {
    const child = responderChild(
      (req) => {
        if (req.method === 'initialize') return { result: { protocolVersion: 1, serverVersion: 'test' } };
        if (req.method === 'shutdown') return { result: null };
        return undefined;
      },
      // le process sort APRÈS avoir envoyé la réponse (docs/rpc-protocol.md)
      (req) => {
        if (req.method === 'shutdown') setTimeout(() => child.emitExit(0), 0);
      },
    );

    const handle = await startServer({
      spawn: () => child.asChildProcess(),
      channel: noopChannel,
      bin: 'vanyline',
    });
    await handle.dispose();

    expect(child.kills).toEqual([]);
  });

  it('dispose : pas de réponse → kill SIGKILL après délai, la promise résout', async () => {
    const child = responderChild(okInitialize); // ne répond jamais à shutdown

    const handle = await startServer({
      spawn: () => child.asChildProcess(),
      channel: noopChannel,
      bin: 'vanyline',
      shutdownTimeoutMs: 50,
    });

    // ne doit pas rejeter (deactivate ne doit jamais jeter)
    await handle.dispose();
    expect(child.kills).toEqual(['SIGKILL']);
  });
});
