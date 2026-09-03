import { createHash } from 'node:crypto';
import { mkdir, mkdtemp, rm, stat, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import {
  EXPECTED_CLI_VERSION,
  ProvisionError,
  ensureCli,
  isExpectedVersion,
  parseSha256File,
  releaseUrls,
  resolveTarget,
  validateFinalUrl,
  type ProvisionDeps,
} from './cli-provisioning';

/** sha256 de la chaîne vide (64 hex minuscules) — hex valide de test. */
const HEX64 = 'e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855';
/** sha256 de « a » (64 hex minuscules) — second hex valide, distinct du premier. */
const HEX64_B = 'ca978112ca1bbdcafac231b39a23dc4da786eff8147c4e72b9807785afee48bb';

const ARCHIVE_NAME = 'vanyline-x86_64-unknown-linux-gnu.tar.gz';

/** Exécute `fn` et retourne l'erreur levée (throw si elle ne lève pas). */
function caught(fn: () => unknown): unknown {
  try {
    fn();
  } catch (err) {
    return err;
  }
  throw new Error('la fonction devait lever une ProvisionError');
}

/** Vérifie que `fn` lève une ProvisionError avec ce code, et retourne l'erreur. */
function expectCode(code: ProvisionError['code'], fn: () => unknown): ProvisionError {
  const err = caught(fn);
  expect(err).toBeInstanceOf(ProvisionError);
  expect((err as ProvisionError).code).toBe(code);
  return err as ProvisionError;
}

describe('resolveTarget', () => {
  it("linux + x64 → 'x86_64-unknown-linux-gnu'", () => {
    expect(resolveTarget('linux', 'x64')).toBe('x86_64-unknown-linux-gnu');
  });

  it("linux + arm64 → 'aarch64-unknown-linux-gnu'", () => {
    expect(resolveTarget('linux', 'arm64')).toBe('aarch64-unknown-linux-gnu');
  });

  it('darwin + x64 → VNL-EXT-003, message nommant platform et arch', () => {
    const err = expectCode('VNL-EXT-003', () => resolveTarget('darwin', 'x64'));
    expect(err.message).toContain('darwin');
    expect(err.message).toContain('x64');
  });

  it('linux + arm → VNL-EXT-003, message nommant platform et arch', () => {
    const err = expectCode('VNL-EXT-003', () => resolveTarget('linux', 'arm'));
    expect(err.message).toContain('linux');
    expect(err.message).toContain('arm');
  });
});

describe('releaseUrls', () => {
  it('URLs exactes archive et .sha256 (v prefixé, même release)', () => {
    const urls = releaseUrls('0.0.11-alpha.5', 'x86_64-unknown-linux-gnu');
    expect(urls.archive).toBe(
      'https://github.com/sebt3/vanyline/releases/download/v0.0.11-alpha.5/vanyline-x86_64-unknown-linux-gnu.tar.gz',
    );
    expect(urls.sha256).toBe(
      'https://github.com/sebt3/vanyline/releases/download/v0.0.11-alpha.5/vanyline-x86_64-unknown-linux-gnu.tar.gz.sha256',
    );
  });
});

describe('parseSha256File', () => {
  it('ligne GNU deux espaces <hex>␣␣<nom> → hex', () => {
    expect(parseSha256File(`${HEX64}  ${ARCHIVE_NAME}\n`, ARCHIVE_NAME)).toBe(HEX64);
  });

  it("variante binaire <hex>␣*<nom> acceptée", () => {
    expect(parseSha256File(`${HEX64} *${ARCHIVE_NAME}\n`, ARCHIVE_NAME)).toBe(HEX64);
  });

  it('plusieurs lignes : la bonne ligne sélectionnée par nom', () => {
    const content =
      `${HEX64_B}  vanyline-aarch64-unknown-linux-gnu.tar.gz\n` +
      `${HEX64}  ${ARCHIVE_NAME}\n`;
    expect(parseSha256File(content, ARCHIVE_NAME)).toBe(HEX64);
  });

  it('hex majuscules → normalisé minuscule', () => {
    expect(parseSha256File(`${HEX64.toUpperCase()}  ${ARCHIVE_NAME}\n`, ARCHIVE_NAME)).toBe(HEX64);
  });

  it('nom d’archive divergent → VNL-EXT-002', () => {
    expectCode('VNL-EXT-002', () =>
      parseSha256File(`${HEX64}  vanyline-aarch64-unknown-linux-gnu.tar.gz\n`, ARCHIVE_NAME),
    );
  });

  it('hex trop court → VNL-EXT-002', () => {
    expectCode('VNL-EXT-002', () => parseSha256File(`abc123  ${ARCHIVE_NAME}\n`, ARCHIVE_NAME));
  });

  it('hex non-hexadécimal → VNL-EXT-002', () => {
    const nonHex = HEX64.replace('e', 'z'); // 64 caractères, non hexadécimal
    expectCode('VNL-EXT-002', () => parseSha256File(`${nonHex}  ${ARCHIVE_NAME}\n`, ARCHIVE_NAME));
  });

  it('contenu vide → VNL-EXT-002', () => {
    expectCode('VNL-EXT-002', () => parseSha256File('', ARCHIVE_NAME));
  });
});

describe('validateFinalUrl', () => {
  it('https://github.com/… → OK', () => {
    expect(() =>
      validateFinalUrl(
        'https://github.com/sebt3/vanyline/releases/download/v0.0.11-alpha.5/vanyline-x86_64-unknown-linux-gnu.tar.gz',
      ),
    ).not.toThrow();
  });

  it('https://objects.githubusercontent.com/… → OK', () => {
    expect(() =>
      validateFinalUrl(
        'https://objects.githubusercontent.com/github-production-release-asset-2e65be/123?foo=bar',
      ),
    ).not.toThrow();
  });

  it('https://media.githubusercontent.com/… → OK (*.githubusercontent.com)', () => {
    expect(() => validateFinalUrl('https://media.githubusercontent.com/media/x/y')).not.toThrow();
  });

  it('http://github.com/… → VNL-EXT-006 (pas de fallback non-https)', () => {
    expectCode('VNL-EXT-006', () =>
      validateFinalUrl('http://github.com/sebt3/vanyline/releases/download/v1/x.tar.gz'),
    );
  });

  it('https://evil.example/… → VNL-EXT-006', () => {
    const err = expectCode('VNL-EXT-006', () => validateFinalUrl('https://evil.example/x.tar.gz'));
    expect(err.message).toContain('evil.example');
  });

  it('https://github.com.evil.example/… → VNL-EXT-006 (suffixe trompeur)', () => {
    expectCode('VNL-EXT-006', () => validateFinalUrl('https://github.com.evil.example/x'));
  });

  it('https://githubusercontent.com/… (sans sous-domaine) → VNL-EXT-006', () => {
    expectCode('VNL-EXT-006', () => validateFinalUrl('https://githubusercontent.com/x'));
  });

  it('chaîne non-URL → VNL-EXT-006 (ne lève pas autre chose)', () => {
    expectCode('VNL-EXT-006', () => validateFinalUrl('pas une url'));
  });
});

describe('isExpectedVersion', () => {
  it("'vanyline 0.0.11-alpha.5' attendu '0.0.11-alpha.5' → true", () => {
    expect(isExpectedVersion('vanyline 0.0.11-alpha.5', '0.0.11-alpha.5')).toBe(true);
  });

  it('\\n final toléré → true', () => {
    expect(isExpectedVersion('vanyline 0.0.11-alpha.5\n', '0.0.11-alpha.5')).toBe(true);
  });

  it('version divergente → false', () => {
    expect(isExpectedVersion('vanyline 0.0.12', '0.0.11-alpha.5')).toBe(false);
  });

  it('autre binaire (kydah) même version → false', () => {
    expect(isExpectedVersion('kydah 0.0.11-alpha.5', '0.0.11-alpha.5')).toBe(false);
  });

  it('sortie vide ou garbage → false sans lever', () => {
    expect(isExpectedVersion('', '0.0.11-alpha.5')).toBe(false);
    expect(isExpectedVersion('garbage', '0.0.11-alpha.5')).toBe(false);
    expect(isExpectedVersion('   \n', '0.0.11-alpha.5')).toBe(false);
  });
});

describe('EXPECTED_CLI_VERSION', () => {
  it('define vitest branché : valeur figée de test', () => {
    expect(EXPECTED_CLI_VERSION).toBe('0.0.0-test');
  });
});

// ---------------------------------------------------------------------------
// ensureCli — deps factices (fetch/execFile) + fs RÉEL dans un home mkdtemp
// injecté. Pas de vrai réseau, pas de vrai tar : le fake 'tar' écrit lui-même
// dest/vanyline ; les Response factices ont un .url contrôlé.
// ---------------------------------------------------------------------------

const TARGET = 'x86_64-unknown-linux-gnu';
const URLS = releaseUrls(EXPECTED_CLI_VERSION, TARGET);

/** Contenu d'archive factice (le fake 'tar' n'en lit pas le contenu). */
const ARCHIVE_BYTES = new TextEncoder().encode('archive-vanyline-factice');
/** sha256 RÉEL d'ARCHIVE_BYTES — pour les cas où l'asset .sha256 est honnête. */
const ARCHIVE_SHA = createHash('sha256').update(ARCHIVE_BYTES).digest('hex');
const SHA256_BODY = `${ARCHIVE_SHA}  ${ARCHIVE_NAME}\n`;

interface FakeReply {
  status?: number;
  body?: BodyInit;
  /** .url de la réponse (défaut : l'URL demandée — github.com, dans l'allowlist). */
  url?: string;
  /** si défini, le fetch rejette cette erreur (simulation réseau). */
  reject?: Error;
}

interface FakeFetch {
  fetch: ProvisionDeps['fetch'];
  /** URLs appelées, dans l'ordre. */
  calls: string[];
}

/** fetch factice : Response dont on contrôle .url ; enregistre l'ordre des URLs ;
 *  une URL non mockée est une erreur de test (attrape les fetchs inattendus). */
function fakeFetch(handlers: Record<string, FakeReply>): FakeFetch {
  const calls: string[] = [];
  const impl = async (input: RequestInfo | URL): Promise<Response> => {
    const url = String(input);
    calls.push(url);
    const reply = handlers[url];
    if (reply === undefined) {
      throw new Error(`fetch inattendu : ${url}`);
    }
    if (reply.reject) throw reply.reject;
    const res = new Response(reply.body ?? '', { status: reply.status ?? 200 });
    Object.defineProperty(res, 'url', { value: reply.url ?? url });
    return res;
  };
  return { fetch: impl as unknown as ProvisionDeps['fetch'], calls };
}

interface ExecCall {
  cmd: string;
  args: readonly string[];
}

interface FakeExec {
  execFile: ProvisionDeps['execFile'];
  calls: ExecCall[];
}

/** execFile factice : stdout de probe '--version' paramétrable ; le fake 'tar'
 *  écrit lui-même dest/vanyline (le -C final de l'argv). */
function fakeExec(versionStdout: string): FakeExec {
  const calls: ExecCall[] = [];
  const execFile: ProvisionDeps['execFile'] = async (cmd, args) => {
    calls.push({ cmd, args });
    if (cmd === 'tar') {
      const dest = args[args.length - 1];
      await writeFile(join(dest, 'vanyline'), '#!/bin/sh\necho vanyline\n', { mode: 0o644 });
      return { stdout: '' };
    }
    return { stdout: versionStdout };
  };
  return { execFile, calls };
}

interface TestDeps extends ProvisionDeps {
  logs: string[];
}

/** deps de base : linux/x64, home et tmp RÉELS injectés, log collecté ; fetch et
 *  execFile « ne doivent pas être appelés » sauf override explicite. */
function baseDeps(home: string, tmp: string, overrides: Partial<ProvisionDeps> = {}): TestDeps {
  const logs: string[] = [];
  return {
    fetch: (async () => {
      throw new Error('fetch ne devait pas être appelé');
    }) as unknown as ProvisionDeps['fetch'],
    execFile: async () => ({ stdout: '' }),
    homedir: () => home,
    tmpdir: () => tmp,
    platform: 'linux',
    arch: 'x64',
    log: (line) => {
      logs.push(line);
    },
    ...overrides,
    logs,
  };
}

async function exists(path: string): Promise<boolean> {
  try {
    await stat(path);
    return true;
  } catch {
    return false;
  }
}

/** attend le rejet ProvisionError `code` et retourne l'erreur. */
async function expectRejectCode(
  code: ProvisionError['code'],
  promise: Promise<unknown>,
): Promise<ProvisionError> {
  try {
    await promise;
  } catch (err) {
    expect(err).toBeInstanceOf(ProvisionError);
    const perr = err as ProvisionError;
    expect(perr.code).toBe(code);
    return perr;
  }
  throw new Error(`ensureCli devait rejeter une ProvisionError ${code}`);
}

describe('ensureCli', () => {
  const binPathIn = (home: string) => join(home, '.local', 'bin', 'vanyline');

  /** pose un binaire existant dans ~/.local/bin du home de test. */
  async function placeBin(home: string): Promise<void> {
    const binDir = join(home, '.local', 'bin');
    await mkdir(binDir, { recursive: true });
    await writeFile(join(binDir, 'vanyline'), '#!/bin/sh\n', { mode: 0o755 });
  }

  let home = '';
  let tmp = '';

  beforeEach(async () => {
    home = await mkdtemp(join(tmpdir(), 'vnl-home-'));
    tmp = await mkdtemp(join(tmpdir(), 'vnl-tmp-'));
  });

  afterEach(async () => {
    await rm(home, { recursive: true, force: true });
    await rm(tmp, { recursive: true, force: true });
  });

  it('cas 1 — serverPath défini : override, aucun probe, aucun fetch', async () => {
    const fetch = fakeFetch({});
    const exec = fakeExec('vanyline 0.0.0-test\n');
    const deps = baseDeps(home, tmp, { fetch: fetch.fetch, execFile: exec.execFile });
    const res = await ensureCli({ serverPath: '/opt/vanyline', autoUpdate: true }, deps);
    expect(res).toEqual({ bin: '/opt/vanyline', source: 'override' });
    expect(fetch.calls).toEqual([]);
    expect(exec.calls).toEqual([]);
    expect(
      deps.logs.some((line) => line.includes('/opt/vanyline') && /auto.?update/i.test(line)),
    ).toBe(true);
  });

  it('cas 2 — binaire existant à jour : cache, aucun fetch', async () => {
    await placeBin(home);
    const fetch = fakeFetch({});
    const exec = fakeExec('vanyline 0.0.0-test\n');
    const deps = baseDeps(home, tmp, { fetch: fetch.fetch, execFile: exec.execFile });
    const res = await ensureCli({ serverPath: '', autoUpdate: true }, deps);
    expect(res).toEqual({ bin: binPathIn(home), source: 'cache' });
    expect(fetch.calls).toEqual([]);
    expect(exec.calls).toEqual([{ cmd: binPathIn(home), args: ['--version'] }]);
  });

  it('cas 3 — version divergente + autoUpdate : install complète, sha PUIS archive (ordre), mode exécutable', async () => {
    await placeBin(home);
    const fetch = fakeFetch({
      [URLS.sha256]: { body: SHA256_BODY },
      [URLS.archive]: { body: ARCHIVE_BYTES },
    });
    const exec = fakeExec('vanyline 0.0.0\n');
    const deps = baseDeps(home, tmp, { fetch: fetch.fetch, execFile: exec.execFile });
    const res = await ensureCli({ serverPath: '', autoUpdate: true }, deps);
    expect(res).toEqual({ bin: binPathIn(home), source: 'installed' });
    expect(fetch.calls).toEqual([URLS.sha256, URLS.archive]); // sha d'abord, archive ensuite
    const tarCall = exec.calls.find((call) => call.cmd === 'tar');
    expect(tarCall).toBeDefined();
    expect(tarCall?.args[0]).toBe('-xf');
    expect(String(tarCall?.args[1]).endsWith(ARCHIVE_NAME)).toBe(true);
    expect(tarCall?.args[2]).toBe('-C');
    const st = await stat(binPathIn(home));
    expect(st.mode & 0o111).not.toBe(0);
  });

  it('cas 4 — binaire absent + autoUpdate : install complète (le fake tar pose dest/vanyline)', async () => {
    const fetch = fakeFetch({
      [URLS.sha256]: { body: SHA256_BODY },
      [URLS.archive]: { body: ARCHIVE_BYTES },
    });
    const exec = fakeExec('');
    const deps = baseDeps(home, tmp, { fetch: fetch.fetch, execFile: exec.execFile });
    const res = await ensureCli({ serverPath: '', autoUpdate: true }, deps);
    expect(res).toEqual({ bin: binPathIn(home), source: 'installed' });
    expect(await exists(binPathIn(home))).toBe(true);
    expect(exec.calls.some((call) => call.cmd === 'tar')).toBe(true);
  });

  it('cas 5 — .sha256 absent (404) : -005, archive jamais téléchargée', async () => {
    const fetch = fakeFetch({ [URLS.sha256]: { status: 404, body: 'Not Found' } });
    const exec = fakeExec('');
    const deps = baseDeps(home, tmp, { fetch: fetch.fetch, execFile: exec.execFile });
    await expectRejectCode('VNL-EXT-005', ensureCli({ serverPath: '', autoUpdate: true }, deps));
    expect(fetch.calls).toEqual([URLS.sha256]);
    expect(exec.calls.filter((call) => call.cmd === 'tar')).toEqual([]);
    expect(await exists(binPathIn(home))).toBe(false);
  });

  it('cas 6 — hash divergent : -002, jamais de tar, rien à binPath', async () => {
    const fetch = fakeFetch({
      // sha déclaré = sha256 de la chaîne vide ≠ sha256 réel d'ARCHIVE_BYTES
      [URLS.sha256]: { body: `${HEX64}  ${ARCHIVE_NAME}\n` },
      [URLS.archive]: { body: ARCHIVE_BYTES },
    });
    const exec = fakeExec('');
    const deps = baseDeps(home, tmp, { fetch: fetch.fetch, execFile: exec.execFile });
    await expectRejectCode('VNL-EXT-002', ensureCli({ serverPath: '', autoUpdate: true }, deps));
    expect(exec.calls.filter((call) => call.cmd === 'tar')).toEqual([]);
    expect(await exists(binPathIn(home))).toBe(false);
  });

  it('cas 7 — fetch qui rejette (TypeError réseau) : -001', async () => {
    const fetch = fakeFetch({ [URLS.sha256]: { reject: new TypeError('fetch failed') } });
    const exec = fakeExec('');
    const deps = baseDeps(home, tmp, { fetch: fetch.fetch, execFile: exec.execFile });
    await expectRejectCode('VNL-EXT-001', ensureCli({ serverPath: '', autoUpdate: true }, deps));
  });

  it('cas 8 — hôte final hors allowlist : -006, rien écrit', async () => {
    const fetch = fakeFetch({
      [URLS.sha256]: { body: SHA256_BODY, url: 'https://evil.example/x' },
    });
    const exec = fakeExec('');
    const deps = baseDeps(home, tmp, { fetch: fetch.fetch, execFile: exec.execFile });
    await expectRejectCode('VNL-EXT-006', ensureCli({ serverPath: '', autoUpdate: true }, deps));
    expect(fetch.calls).toEqual([URLS.sha256]);
    expect(await exists(binPathIn(home))).toBe(false);
  });

  it('cas 9 — ~/.local est un fichier régulier : mkdir échoue → -004', async () => {
    await writeFile(join(home, '.local'), 'pas un dossier\n');
    const fetch = fakeFetch({}); // ne doit jamais être appelé (mkdir avant fetch)
    const deps = baseDeps(home, tmp, { fetch: fetch.fetch });
    await expectRejectCode('VNL-EXT-004', ensureCli({ serverPath: '', autoUpdate: true }, deps));
    expect(fetch.calls).toEqual([]);
  });

  it('cas 10 — autoUpdate false + version divergente : cache, aucun fetch', async () => {
    await placeBin(home);
    const fetch = fakeFetch({});
    const exec = fakeExec('vanyline 0.0.0\n');
    const deps = baseDeps(home, tmp, { fetch: fetch.fetch, execFile: exec.execFile });
    const res = await ensureCli({ serverPath: '', autoUpdate: false }, deps);
    expect(res).toEqual({ bin: binPathIn(home), source: 'cache' });
    expect(fetch.calls).toEqual([]);
  });

  it("cas 11 — platform 'darwin' : -003 avant tout fetch et toute écriture dans le home", async () => {
    const fetch = fakeFetch({});
    const deps = baseDeps(home, tmp, { platform: 'darwin', fetch: fetch.fetch });
    await expectRejectCode('VNL-EXT-003', ensureCli({ serverPath: '', autoUpdate: true }, deps));
    expect(fetch.calls).toEqual([]);
    expect(await exists(join(home, '.local'))).toBe(false);
  });
});
