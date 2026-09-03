import type { ChildProcess, SpawnOptions } from 'node:child_process';
import {
  PROTOCOL_VERSION,
  RpcConnection,
  RpcError,
  vnlCode,
  type InitializeParams,
  type InitializeResult,
  type RpcTransport,
} from '@vanyline/protocol';

/** Transport ndjson branché sur les stdio d'un child process. write() ajoute le \n ;
 *  onLine() renvoie des lignes complètes (buffering des fragments). */
export function createStdioTransport(child: Pick<ChildProcess, 'stdin' | 'stdout'>): RpcTransport {
  const callbacks: Array<(line: string) => void> = [];
  let buffer = '';

  child.stdout?.on('data', (chunk: Buffer) => {
    buffer += chunk.toString('utf8');
    let idx = buffer.indexOf('\n');
    while (idx >= 0) {
      const line = buffer.slice(0, idx);
      buffer = buffer.slice(idx + 1);
      for (const cb of callbacks) {
        cb(line);
      }
      idx = buffer.indexOf('\n');
    }
  });

  return {
    write(line: string): void {
      child.stdin?.write(`${line}\n`);
    },
    onLine(cb: (line: string) => void): void {
      callbacks.push(cb);
    },
  };
}

/** Erreurs du cycle de vie serveur, avec identifiant unique (règle AGENTS.md). */
export class ServerError extends Error {
  /** 'VNL-EXT-010' (spawn échoué / binaire introuvable),
   *  'VNL-EXT-011' (initialize échoué — mismatch de protocole ou autre). */
  readonly vnlExtCode: string;
  /** code VNL-RPC-* serveur si connu (ex. 'VNL-RPC-003'), sinon undefined. */
  readonly serverCode?: string;

  constructor(vnlExtCode: string, message: string, serverCode?: string) {
    super(message);
    this.name = 'ServerError';
    this.vnlExtCode = vnlExtCode;
    this.serverCode = serverCode;
    Object.setPrototypeOf(this, ServerError.prototype);
  }
}

/** OutputChannel structural — le vrai type vscode satisfaction triviale, testable. */
export interface LogChannel {
  appendLine(value: string): void;
}

export interface StartServerDeps {
  /** injectable pour les tests (sinon child_process.spawn). */
  spawn: (bin: string, args: string[], options: SpawnOptions) => ChildProcess;
  channel: LogChannel;
  /** Binaire résolu (chemin absolu ou nom dans le PATH). */
  bin: string;
  /** workspaceFolders[0]?.uri.fsPath — passé tel quel à initialize et comme cwd. */
  workspace?: string;
  /** Niveau RUST_LOG passé à l'enfant ; absent → ne pas écraser un RUST_LOG hérité. */
  logLevel?: string;
  /** Défaut 3000 : délai d'attente de la réponse `shutdown` avant kill forcé. */
  shutdownTimeoutMs?: number;
}

export interface ServerHandle {
  conn: RpcConnection;
  child: ChildProcess;
  /** shutdown (avec délai) puis kill de secours ; toujours kill si encore vivant. */
  dispose(): Promise<void>;
}

type InitOutcome =
  | { kind: 'initialized'; result: InitializeResult }
  | { kind: 'rpc-error'; err: unknown }
  | { kind: 'child-exit' }
  | { kind: 'child-error'; err: unknown };

/**
 * spawn + stdio + initialize. Résout quand initialize a répondu ; rejette ServerError sinon.
 * - spawn : argv ['serve', '--stdio'], shell:false, cwd: workspace (si défini).
 * - stderr : chaque ligne → channel.appendLine. stdout : réservé au protocole.
 * - env : { ...process.env, ...(logLevel ? { RUST_LOG: logLevel } : {}) }.
 * - initialize({ protocolVersion: PROTOCOL_VERSION, workspace }) (workspace omis si undefined).
 * - Événements 'error'/'exit' de l'enfant avant la fin de initialize → ServerError
 *   VNL-EXT-010 / VNL-EXT-011, conn.close() + kill (pas de pending laissé).
 *   Après la résolution, 'error'/'exit' relèvent du superviseur (tâche 02b).
 */
export async function startServer(deps: StartServerDeps): Promise<ServerHandle> {
  const env: NodeJS.ProcessEnv = {
    ...process.env,
    ...(deps.logLevel ? { RUST_LOG: deps.logLevel } : {}),
  };

  let child: ChildProcess;
  try {
    child = deps.spawn(deps.bin, ['serve', '--stdio'], {
      cwd: deps.workspace,
      env,
      shell: false,
    });
  } catch (err) {
    throw new ServerError(
      'VNL-EXT-010',
      `VNL-EXT-010: Échec du lancement de « ${deps.bin} » (${String(err)}) — ` +
        'vérifiez le paramètre vanyline.serverPath ou installez vanyline dans le PATH.',
    );
  }

  // stderr : logs de la CLI, ligne par ligne (stdout réservé au protocole).
  child.stderr?.on('data', (chunk: Buffer) => {
    for (const line of chunk.toString('utf8').split('\n')) {
      if (line.length > 0) {
        deps.channel.appendLine(line);
      }
    }
  });

  const conn = new RpcConnection(createStdioTransport(child), {
    // Les tours LLM de chat/send dépassent largement le défaut de 10 s ;
    // la granularité par-requête est un affinement éventuel de 04.
    timeoutMs: 300_000,
  });

  let childExited = false;
  const exitPromise = new Promise<void>((resolve) => {
    child.once('exit', () => {
      childExited = true;
      resolve();
    });
  });
  const childErrorPromise = new Promise<{ err: unknown }>((resolve) => {
    child.once('error', (err: unknown) => resolve({ err }));
  });

  const params: InitializeParams = {
    protocolVersion: PROTOCOL_VERSION,
    ...(deps.workspace !== undefined ? { workspace: deps.workspace } : {}),
  };

  const outcome = await Promise.race<InitOutcome>([
    conn
      .request<InitializeResult>('initialize', params)
      .then((result): InitOutcome => ({ kind: 'initialized', result }))
      .catch((err: unknown): InitOutcome => ({ kind: 'rpc-error', err })),
    exitPromise.then((): InitOutcome => ({ kind: 'child-exit' })),
    childErrorPromise.then(({ err }): InitOutcome => ({ kind: 'child-error', err })),
  ]);

  if (outcome.kind !== 'initialized') {
    // Pas de pending laissé derrière soi + kill de secours (l'enfant est peut-être déjà mort).
    conn.close();
    child.kill('SIGKILL');
  }

  switch (outcome.kind) {
    case 'child-error': {
      const code = (outcome.err as NodeJS.ErrnoException | undefined)?.code ?? String(outcome.err);
      throw new ServerError(
        'VNL-EXT-010',
        `VNL-EXT-010: Échec du lancement de « ${deps.bin} » (${code}) — ` +
          'vérifiez le paramètre vanyline.serverPath ou installez vanyline dans le PATH.',
      );
    }
    case 'child-exit': {
      throw new ServerError(
        'VNL-EXT-011',
        `VNL-EXT-011: le serveur vanyline « ${deps.bin} » s'est arrêté pendant ` +
          `initialize (code ${child.exitCode ?? 'inconnu'}).`,
      );
    }
    case 'rpc-error': {
      const err = outcome.err;
      if (err instanceof RpcError && err.vnlCode === vnlCode.UNKNOWN_PROTOCOL_VERSION) {
        throw new ServerError(
          'VNL-EXT-011',
          `VNL-EXT-011: protocole incompatible entre l'extension vanyline ` +
            `(protocole ${PROTOCOL_VERSION}) et le binaire « ${deps.bin} » (${err.message}). ` +
            'Mettez à jour l\'extension vanyline ou le binaire vanyline.',
          vnlCode.UNKNOWN_PROTOCOL_VERSION,
        );
      }
      throw new ServerError(
        'VNL-EXT-011',
        `VNL-EXT-011: initialize a échoué (${err instanceof Error ? err.message : String(err)}).`,
      );
    }
    case 'initialized':
      break;
  }

  const shutdownTimeoutMs = deps.shutdownTimeoutMs ?? 3000;

  const dispose = async (): Promise<void> => {
    if (childExited) {
      conn.close();
      return;
    }
    // shutdown puis sortie « volontaire » de l'enfant ; kill SIGKILL de secours.
    // Ne rejette jamais : deactivate ne doit pas jeter.
    const grace = new Promise<void>((resolve) => {
      setTimeout(resolve, shutdownTimeoutMs).unref();
    });
    try {
      await Promise.race([conn.request('shutdown').then(() => exitPromise), grace]);
    } catch {
      // réponse d'erreur ou request rejetée après close() : kill ci-dessous.
    }
    conn.close();
    if (!childExited) {
      child.kill('SIGKILL');
    }
  };

  return { conn, child, dispose };
}
