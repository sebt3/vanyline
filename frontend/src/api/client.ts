/** Erreur normalisée depuis le corps JSON `{"error": "..."}` renvoyé par toutes les routes app. */
export class ApiError extends Error {
  status: number;
  code: string | undefined;

  constructor(
    status: number,
    code: string | undefined,
    message: string,
  ) {
    super(message);
    this.name = 'ApiError';
    this.status = status;
    this.code = code;
  }
}

export interface ApiClient {
  get<T>(path: string): Promise<T>;
  post<T>(path: string, body?: unknown): Promise<T>;
  put<T>(path: string, body?: unknown): Promise<T>;
  delete(path: string): Promise<void>;
}

/** Crée un client HTTP vers l'app. `baseUrl` vide (défaut) → chemins relatifs `/api/…`,
 *  valides en prod (same-origin, `app` sert le frontend) et en dev (proxy Vite). */
export function createApiClient(baseUrl?: string): ApiClient {
  return {
    async get<T>(path: string): Promise<T> {
      return request<T>('GET', path, undefined, baseUrl);
    },

    async post<T>(path: string, body?: unknown): Promise<T> {
      return request<T>('POST', path, body, baseUrl);
    },

    async put<T>(path: string, body?: unknown): Promise<T> {
      return request<T>('PUT', path, body, baseUrl);
    },

    async delete(path: string): Promise<void> {
      return requestVoid('DELETE', path, undefined, baseUrl);
    },
  };
}

async function request<T>(
  method: string,
  path: string,
  body: unknown | undefined,
  baseUrl: string | undefined,
): Promise<T> {
  const hasBody = method === 'POST' || method === 'PUT';
  const headers: Record<string, string> = {};

  if (hasBody && body !== undefined) {
    headers['Content-Type'] = 'application/json';
  }

  const init: RequestInit = {
    method,
    credentials: 'include',
    headers,
  };

  if (hasBody && body !== undefined) {
    init.body = JSON.stringify(body);
  }

  const url = baseUrl ? `${baseUrl}${path}` : path;
  let response: Response;

  try {
    response = await globalThis.fetch(url, init);
  } catch {
    const msg = `Network error: ${path}`;
    throw new ApiError(0, undefined, msg);
  }

  if (response.status === 401) {
    redirectToLogin();
  }

  if (!response.ok) {
    const contentType = response.headers.get('content-type') ?? '';
    let error: string;
    let code: string | undefined;

    if (contentType.includes('application/json')) {
      try {
        const json = await response.json();
        error = typeof json.error === 'string' ? json.error : JSON.stringify(json);
        code = extractCode(error);
      } catch {
        const msg = `HTTP ${response.status}`;
        throw new ApiError(response.status, undefined, msg);
      }
    } else {
      const msg = `HTTP ${response.status}`;
      throw new ApiError(response.status, undefined, msg);
    }
    throw new ApiError(response.status, code, error);
  }

  return (await response.json()) as T;
}

async function requestVoid(
  method: string,
  path: string,
  body: unknown | undefined,
  baseUrl: string | undefined,
): Promise<void> {
  const hasBody = method === 'POST' || method === 'PUT';
  const headers: Record<string, string> = {};

  if (hasBody && body !== undefined) {
    headers['Content-Type'] = 'application/json';
  }

  const init: RequestInit = {
    method,
    credentials: 'include',
    headers,
  };

  if (hasBody && body !== undefined) {
    init.body = JSON.stringify(body);
  }

  const url = baseUrl ? `${baseUrl}${path}` : path;

  try {
    const response = await globalThis.fetch(url, init);
    if (response.status === 401) {
      redirectToLogin();
    }
    if (!response.ok) {
      const contentType = response.headers.get('content-type') ?? '';
      let error: string;
      let code: string | undefined;

      if (contentType.includes('application/json')) {
        try {
          const json = await response.json();
          error = typeof json.error === 'string' ? json.error : JSON.stringify(json);
          code = extractCode(error);
        } catch {
          const msg = `HTTP ${response.status}`;
          throw new ApiError(response.status, undefined, msg);
        }
        throw new ApiError(response.status, code, error);
      }
      const msg = `HTTP ${response.status}`;
      throw new ApiError(response.status, undefined, msg);
    }
    // 204 NO_CONTENT et autres réponses sans corps : ok, void
  } catch (err) {
    if (err instanceof ApiError) {
      throw err;
    }
    const msg = `Network error: ${path}`;
    throw new ApiError(0, undefined, msg);
  }
}

/** Session cookie absente/expirée : seule issue possible côté SPA, on renvoie
 *  vers le flow OIDC (`app` redirige lui-même vers Authentik). */
function redirectToLogin(): void {
  globalThis.location.href = '/auth/login';
}

/** Extrait `VNL-XXX-\d+` d'une chaîne de message, sinon `undefined`. */
function extractCode(error: string): string | undefined {
  const match = error.match(/VNL-[\w-]+-\d+/);
  return match?.[0];
}