import type { JsonRpcResponse } from './rpc';

/** Transport ndjson minimal — injecté par l'appelant (CLI stdio, socket,
 *  worker...). `write` envoie une ligne complète (SANS `\n`), `onLine` en
 *  reçoit une ligne complète (déjà sans `\n`). */
export interface RpcTransport {
  write(line: string): void;
  onLine(cb: (line: string) => void): void;
}

export interface RpcConnectionOptions {
  /** Délai max d'une `request` avant rejet `RpcTimeoutError`. Défaut 10_000. */
  timeoutMs?: number;
}

/** Rejet sur délai dépassé d'une `request` (pas une erreur JSON-RPC serveur). */
export class RpcTimeoutError extends Error {
  constructor(timeoutMs: number) {
    super(`Request timed out after ${timeoutMs}ms`);
    this.name = 'RpcTimeoutError';
  }
}

/** Rejet quand le serveur répond avec un champ `error` JSON-RPC. */
export class RpcError extends Error {
  readonly code: number;
  readonly vnlCode?: string;
  constructor(code: number, message: string, vnlCode?: string) {
    super(message);
    this.name = 'RpcError';
    this.code = code;
    this.vnlCode = vnlCode;
    Object.setPrototypeOf(this, RpcError.prototype);
  }
}

interface PendingRequest {
  resolve: (value: unknown) => void;
  reject: (reason: unknown) => void;
  timer: ReturnType<typeof setTimeout>;
}

export class RpcConnection {
  private readonly transport: RpcTransport;
  private readonly timeoutMs: number;
  private pending: Map<number | string, PendingRequest> = new Map();
  private nextId = 1;
  private notificationHandlers: Map<string, Array<(params: unknown) => void>> = new Map();
  private closed = false;

  constructor(transport: RpcTransport, options?: RpcConnectionOptions) {
    this.transport = transport;
    this.timeoutMs = options?.timeoutMs ?? 10_000;

    this.transport.onLine((line: string) => {
      if (this.closed) return;
      this.handleLine(line);
    });
  }

  private handleLine(line: string): void {
    let parsed: unknown;
    try {
      parsed = JSON.parse(line);
    } catch {
      // JSON invalide → ignorée
      return;
    }

    const obj = parsed as Record<string, unknown>;

    // Réponse : ligne avec `id`
    if ('id' in obj) {
      const id = obj.id as number | string;
      const pending = this.pending.get(id);
      if (!pending) {
        // id inconnu → ignorée
        return;
      }
      this.pending.delete(id);
      clearTimeout(pending.timer);

      if (obj.error) {
        const errData = obj.error as { code?: number; message?: string; data?: { code?: string } };
        const rpcErr = new RpcError(
          errData.code ?? -32600,
          errData.message ?? 'Unknown error',
          errData.data?.code,
        );
        pending.reject(rpcErr);
      } else {
        pending.resolve(obj.result);
      }
      return;
    }

    // Notification : ligne avec `method` et SANS `id`
    if ('method' in obj && obj.method !== undefined) {
      const method = obj.method as string;
      const params = obj.params ?? null;
      const handlers = this.notificationHandlers.get(method);
      if (handlers) {
        for (const handler of handlers) {
          handler(params);
        }
      }
      return;
    }
  }

  request<T = unknown>(method: string, params?: unknown): Promise<T> {
    if (this.closed) {
      return Promise.reject(new RpcTimeoutError(this.timeoutMs));
    }

    const id = this.nextId++;
    const body: Record<string, unknown> = {
      jsonrpc: '2.0' as const,
      id,
      method,
    };
    if (params !== undefined) {
      body.params = params;
    }
    this.transport.write(JSON.stringify(body));

    return new Promise<T>((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new RpcTimeoutError(this.timeoutMs));
      }, this.timeoutMs) as unknown as ReturnType<typeof setTimeout>;

      // Overwrite timer reference so clearTimeout works properly
      const origReject = reject;
      this.pending.set(id, {
        resolve(value) {
          clearTimeout(timer);
          resolve(value as T);
        },
        reject(reason) {
          clearTimeout(timer);
          origReject(reason);
        },
        timer,
      });
    });
  }

  onNotification(method: string, handler: (params: unknown) => void): void {
    const handlers = this.notificationHandlers.get(method) ?? [];
    handlers.push(handler);
    this.notificationHandlers.set(method, handlers);
  }

  close(): void {
    this.closed = true;
    for (const [id, pending] of this.pending) {
      this.pending.delete(id);
      clearTimeout(pending.timer);
      pending.reject(new RpcTimeoutError(this.timeoutMs));
    }
  }
}