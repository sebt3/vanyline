import { createApiClient } from './client';

const client = createApiClient();

export type FileState =
  | 'modified' | 'added' | 'deleted' | 'renamed' | 'untracked' | 'conflicted';

export interface FileEntry {
  path: string;
  state: FileState;
  staged: boolean;
}

export interface GitStatus {
  branch: string;
  files: FileEntry[];
  clean: boolean;
}

export interface DiffResult {
  path: string;
  diff: string;
}

export interface CommitResult {
  sha: string;
  title: string;
}

export interface PushResult {
  ok: boolean;
  pushed: number;
}

export interface BranchEntry {
  name: string;
  is_remote: boolean;
  upstream: string | null;
}

export interface BranchesResult {
  current: string;
  merging: boolean;
  branches: BranchEntry[];
}

export interface LogCommit {
  sha: string;
  parents: string[];
  refs: string[];
  title: string;
  author: string;
  date: string;
}

export interface LogResult {
  branch: string;
  commits: LogCommit[];
  truncated: boolean;
}

export interface MergeResult {
  conflicted: boolean;
  sha: string | null;
}

export interface SshKeyStatus {
  exists: boolean;
  public_key: string | null;
}

export interface SshKeyResult {
  public_key: string;
}

/** Base URL du relais git : `/api/sandboxes/{name}/git` (name encodé). */
const base = (name: string): string =>
  `/api/sandboxes/${encodeURIComponent(name)}/git`;

export const gitClient = {
  /** GET /git/status */
  status(name: string): Promise<GitStatus> {
    return client.get(`${base(name)}/status`);
  },

  /** GET /git/diff?path=…[&staged=true] — `path` encodé, `staged` optionnel */
  diff(name: string, path: string, staged?: boolean): Promise<DiffResult> {
    const stagedParam = staged ? '&staged=true' : '';
    return client.get(
      `${base(name)}/diff?path=${encodeURIComponent(path)}${stagedParam}`,
    );
  },

  /** POST /git/stage { paths } */
  stage(name: string, paths: string[]): Promise<{ ok: boolean }> {
    return client.post(`${base(name)}/stage`, { paths });
  },

  /** POST /git/unstage { paths } */
  unstage(name: string, paths: string[]): Promise<{ ok: boolean }> {
    return client.post(`${base(name)}/unstage`, { paths });
  },

  /** POST /git/commit { message } */
  commit(name: string, message: string): Promise<CommitResult> {
    return client.post(`${base(name)}/commit`, { message });
  },

  /** POST /git/push (pas de body) */
  push(name: string): Promise<PushResult> {
    return client.post(`${base(name)}/push`);
  },

  /** GET /git/branches */
  branches(name: string): Promise<BranchesResult> {
    return client.get(`${base(name)}/branches`);
  },

  /** POST /git/branches { name, from? } */
  createBranch(name: string, branchName: string, from?: string): Promise<{ ok: boolean }> {
    return client.post(`${base(name)}/branches`, { name: branchName, from });
  },

  /** POST /git/checkout { branch } */
  checkout(name: string, branch: string): Promise<{ ok: boolean }> {
    return client.post(`${base(name)}/checkout`, { branch });
  },

  /** DELETE /git/branches/{branch} */
  deleteBranch(name: string, branch: string): Promise<void> {
    return client.delete(`${base(name)}/branches/${encodeURIComponent(branch)}`);
  },

  /** POST /git/merge { branch } — conflit = réponse 200 { conflicted: true } */
  merge(name: string, branch: string): Promise<MergeResult> {
    return client.post(`${base(name)}/merge`, { branch });
  },

  /** POST /git/merge/abort (pas de body) */
  mergeAbort(name: string): Promise<{ ok: boolean }> {
    return client.post(`${base(name)}/merge/abort`);
  },

  /** GET /git/log?limit=…[&all=true] — params optionnels */
  log(name: string, limit?: number, all?: boolean): Promise<LogResult> {
    const params = new URLSearchParams();
    if (limit !== undefined) params.set('limit', String(limit));
    if (all !== undefined) params.set('all', String(all));
    const qs = params.toString();
    return client.get(`${base(name)}/log${qs ? `?${qs}` : ''}`);
  },

  /** GET /git/ssh-key */
  sshKeyStatus(name: string): Promise<SshKeyStatus> {
    return client.get(`${base(name)}/ssh-key`);
  },

  /** POST /git/ssh-key (pas de body) */
  sshKeyCreate(name: string): Promise<SshKeyResult> {
    return client.post(`${base(name)}/ssh-key`);
  },
};