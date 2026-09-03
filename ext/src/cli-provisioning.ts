import { mkdir, copyFile, rename, chmod, mkdtemp, rm, stat, writeFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { tmpdir as osTmpdir, homedir as osHomedir } from 'node:os';
import { join, basename } from 'node:path';
import { execFile as nodeExecFile } from 'node:child_process';

declare const __EXPECTED_CLI_VERSION__: string; // esbuild define (tâche 03a) + define vitest

/** Version CLI attendue, injectée au build depuis ext/cli-version.txt. */
export const EXPECTED_CLI_VERSION: string = __EXPECTED_CLI_VERSION__;

/** Erreur de provisioning avec identifiant unique (règle AGENTS.md).
 *  Codes : -001 pas de réseau ; -002 checksum invalide (absent du contenu, format, ou
 *  nom d'archive divergent — le fichier lui-même est « absent » = -005) ; -003 target non
 *  supporté ; -004 ~/.local/bin non inscriptible ; -005 asset .sha256 absent (404) ;
 *  -006 téléchargement refusé (redirect vers un hôte hors allowlist / protocole non-https). */
export class ProvisionError extends Error {
  readonly code:
    | 'VNL-EXT-001'
    | 'VNL-EXT-002'
    | 'VNL-EXT-003'
    | 'VNL-EXT-004'
    | 'VNL-EXT-005'
    | 'VNL-EXT-006';

  constructor(code: ProvisionError['code'], message: string) {
    super(message);
    this.name = 'ProvisionError';
    this.code = code;
    Object.setPrototypeOf(this, ProvisionError.prototype);
  }
}

/** linux+x64 → 'x86_64-unknown-linux-gnu' ; linux+arm64 → 'aarch64-unknown-linux-gnu' ;
 *  sinon throw ProvisionError VNL-EXT-003 (message nommant platform/arch).
 *  (design : Linux x86_64 + aarch64 uniquement, pas de macOS/Windows.) */
export function resolveTarget(platform: NodeJS.Platform, arch: string): string {
  if (platform === 'linux') {
    if (arch === 'x64') return 'x86_64-unknown-linux-gnu';
    if (arch === 'arm64') return 'aarch64-unknown-linux-gnu';
  }
  throw new ProvisionError(
    'VNL-EXT-003',
    `Plateforme non supportée : ${platform}/${arch} — ` +
      'seuls Linux x86_64 et aarch64 sont fournis en release.',
  );
}

/** https strict, séparateurs de version déjà « v »-préfixés :
 *  archive → https://github.com/sebt3/vanyline/releases/download/v<version>/vanyline-<target>.tar.gz
 *  sha256  → la même chose + '.sha256' */
export function releaseUrls(version: string, target: string): { archive: string; sha256: string } {
  const archive =
    `https://github.com/sebt3/vanyline/releases/download/v${version}/vanyline-${target}.tar.gz`;
  return { archive, sha256: `${archive}.sha256` };
}

/** Ligne d'un fichier sha256sum GNU : `<hex64>␣␣<nom>` (mode texte) ou `<hex64>␣*<nom>`
 *  (mode binaire). L'hex doit faire exactement 64 caractères hexadécimaux. */
const SHA256_LINE = /^([0-9a-fA-F]{64}) +\*?(\S+)$/;

/** Analyse le contenu de l'asset .sha256 (format `<hex64>  <nom>` ou `<hex64> *<nom>`,
 *  GNU coreutils). Cherche la ligne dont le nom correspond à expectedArchiveName
 *  (comparaison basename stricte), retourne le hex minuscule.
 *  Aucune ligne ne matche (fichier incohérent, mauvais nom d'archive, hex invalide)
 *  → throw ProvisionError VNL-EXT-002. */
export function parseSha256File(content: string, expectedArchiveName: string): string {
  for (const line of content.split('\n')) {
    const match = SHA256_LINE.exec(line.trim());
    if (!match) continue;
    const [, hex, name] = match;
    const basename = name.split('/').pop() ?? name;
    if (basename === expectedArchiveName) {
      return hex.toLowerCase();
    }
  }
  throw new ProvisionError(
    'VNL-EXT-002',
    `Checksum SHA256 absent ou invalide pour « ${expectedArchiveName} » dans l'asset .sha256.`,
  );
}

/** Contrôle de l'URL FINALE après redirects (design : allowlist github.com,
 *  objects.githubusercontent.com, *.githubusercontent.com — refus sinon).
 *  NB : `fetch` suit les redirects tout seul, donc la requête a DÉJÀ atteint
 *  l'hôte final (et son corps est en main) quand on valide. L'allowlist est donc
 *  un garde-fou intégrité / anti-exfiltration, pas une garantie « on ne parle
 *  jamais à un hôte non-GitHub » — l'archive reste de toute façon SHA256-gatée.
 *  Accepte uniquement protocole https: ; host = 'github.com' | 'objects.githubusercontent.com'
 *  | se terminant par '.githubusercontent.com' (avec point, ex. foo.githubusercontent.com ;
 *  githubusercontent.com nu REFOUSÉ). Sinon throw ProvisionError VNL-EXT-006
 *  (message avec l'hôte). URL invalide → VNL-EXT-006 aussi. */
export function validateFinalUrl(finalUrl: string): void {
  let url: URL;
  try {
    url = new URL(finalUrl);
  } catch {
    throw new ProvisionError('VNL-EXT-006', `URL de téléchargement invalide : « ${finalUrl} ».`);
  }
  const host = url.hostname.toLowerCase();
  const allowed =
    url.protocol === 'https:' &&
    (host === 'github.com' ||
      host === 'objects.githubusercontent.com' ||
      host.endsWith('.githubusercontent.com'));
  if (!allowed) {
    throw new ProvisionError(
      'VNL-EXT-006',
      `Téléchargement refusé : hôte « ${host} » hors allowlist (https, github.com / *.githubusercontent.com uniquement).`,
    );
  }
}

/** Sortie de `vanyline --version` (ex. 'vanyline 0.0.11-alpha.5' + \n éventuel) :
 *  vrai ssi le 2ᵉ jeton == expected (le 1ᵉʳ jeton doit être 'vanyline'). */
export function isExpectedVersion(versionOutput: string, expected: string): boolean {
  const tokens = versionOutput.split(/\s+/).filter((token) => token.length > 0);
  return tokens[0] === 'vanyline' && tokens[1] === expected;
}

// ---------------------------------------------------------------------------
// Couche I/O (tâche 03b) — ensureCli. Les fs node:fs/promises sont réels ;
// réseau, process et environnement passent par ProvisionDeps (testabilité).
// ---------------------------------------------------------------------------

/** Dépendances injectables (testabilité). La version de production passe les vraies impls. */
export interface ProvisionDeps {
  fetch: typeof globalThis.fetch;
  /** execFile promis, argv strict (jamais de shell). Rejet = erreur (code dans .code/.message). */
  execFile: (cmd: string, args: readonly string[]) => Promise<{ stdout: string }>;
  homedir: () => string;
  tmpdir: () => string;
  platform: NodeJS.Platform;
  arch: string;
  log: (line: string) => void;
}

export interface ResolvedCli {
  readonly bin: string;
  readonly source: 'override' | 'cache' | 'installed';
}

export interface EnsureCliConfig {
  /** config vanyline.serverPath (déjà trim côté appelant ou ici, indifférent) */
  readonly serverPath: string;
  /** config vanyline.autoUpdateCli (défaut true) */
  readonly autoUpdate: boolean;
}

/** Tout échec d'écriture (disk write / mkdir / copy / rename) est -004 :
 *  « destination non inscriptible », quel que soit l'errno (EACCES, EPERM,
 *  ENOTDIR, EISDIR… — contrat 03b). */
function writeError(err: unknown, target: string): ProvisionError {
  const reason = err instanceof Error ? err.message : String(err);
  return new ProvisionError('VNL-EXT-004', `VNL-EXT-004: écriture vers « ${target} » impossible (${reason}).`);
}

/** Rejet du fetch lui-même (pas de réseau / DNS / TLS) → -001. */
function networkError(err: unknown, url: string): ProvisionError {
  if (err instanceof ProvisionError) return err;
  const reason = err instanceof Error ? err.message : String(err);
  return new ProvisionError('VNL-EXT-001', `VNL-EXT-001: téléchargement impossible (${url}) (${reason}).`);
}

/** Récupère l'asset .sha256 de la même release et retourne le hex attendu.
 *  404 → -005 (refus, pas de fallback) ; autre !ok → -001 ; hôte final hors
 *  allowlist → -006 (validateFinalUrl) ; contenu incohérent → -002. */
async function fetchExpectedSha256(
  deps: ProvisionDeps,
  url: string,
  archiveName: string,
): Promise<string> {
  let res: Response;
  try {
    res = await deps.fetch(url);
  } catch (err) {
    throw networkError(err, url);
  }
  if (res.status === 404) {
    throw new ProvisionError(
      'VNL-EXT-005',
      `VNL-EXT-005: asset .sha256 absent (HTTP 404) : ${url} — ` +
        'téléchargement refusé (aucun fallback sans vérification d’intégrité).',
    );
  }
  if (!res.ok) {
    throw new ProvisionError(
      'VNL-EXT-001',
      `VNL-EXT-001: téléchargement du .sha256 refusé (HTTP ${res.status}) : ${url}.`,
    );
  }
  validateFinalUrl(res.url);
  return parseSha256File(await res.text(), archiveName);
}

/**
 * Orchestration complète du design « Provisioning CLI ». Ne rejette qu'avec
 * ProvisionError (-001..-006). Ne spawn JAMAIS le binaire téléchargé (le start du
 * superviseur le fera après cette fonction).
 */
export async function ensureCli(
  cfg: EnsureCliConfig,
  deps: ProvisionDeps,
): Promise<ResolvedCli> {
  // 1. serverPath (après trim) → utilisé tel quel, auto-update désactivée,
  //    aucun probe, aucun fetch (design : « log clair du binaire utilisé »).
  const serverPath = cfg.serverPath.trim();
  if (serverPath.length > 0) {
    deps.log(
      `vanyline.serverPath défini : « ${serverPath} » utilisé tel quel — auto-update désactivée.`,
    );
    return { bin: serverPath, source: 'override' };
  }

  // 2. cible plateforme/arch — -003 levé avant tout I/O disque.
  const target = resolveTarget(deps.platform, deps.arch);

  // 3. emplacement géré : ~/.local/bin/vanyline.
  const binDir = join(deps.homedir(), '.local', 'bin');
  const binPath = join(binDir, 'vanyline');

  // 4. probe du cache : toute erreur execFile (ou stat) = version invalide,
  //    jamais de rejet ici.
  let cached = false;
  try {
    await stat(binPath);
    cached = true;
  } catch {
    cached = false; // absent (ou chemin non résoluble) : pas de cache
  }
  if (cached) {
    let stdout = '';
    try {
      ({ stdout } = await deps.execFile(binPath, ['--version']));
    } catch {
      stdout = ''; // erreur d'exécution du probe → version invalide (contrat)
    }
    if (isExpectedVersion(stdout, EXPECTED_CLI_VERSION)) {
      deps.log(`binaire vanyline à jour (${EXPECTED_CLI_VERSION}) : ${binPath}`);
      return { bin: binPath, source: 'cache' };
    }
  }

  // 5. auto-update désactivée : on garde le cache tel quel. Un binaire absent
  //    fera échouer le spawn côté rpc.ts (-010) — mode dégradé du superviseur.
  if (!cfg.autoUpdate) {
    deps.log(
      cached
        ? `version divergente et auto-update désactivée : ${binPath} (aucune installation)`
        : `binaire absent et auto-update désactivée : ${binPath} (aucune installation)`,
    );
    return { bin: binPath, source: 'cache' };
  }

  // 6. download : .sha256 OBLIGATOIRE d'abord, archive ensuite, hash vérifié
  //    avant la moindre écriture de binaire dans ~/.local/bin (seul le mkdir
  //    de destination a pu créer le dossier à ce stade — aucun binaire posé).
  const urls = releaseUrls(EXPECTED_CLI_VERSION, target);
  const archiveName = basename(urls.archive);

  try {
    await mkdir(binDir, { recursive: true });
  } catch (err) {
    throw writeError(err, binDir);
  }

  const expected = await fetchExpectedSha256(deps, urls.sha256, archiveName);

  let bytes: Buffer;
  try {
    const res = await deps.fetch(urls.archive);
    if (!res.ok) {
      throw new ProvisionError(
        'VNL-EXT-001',
        `VNL-EXT-001: téléchargement de l’archive refusé (HTTP ${res.status}) : ${urls.archive}.`,
      );
    }
    validateFinalUrl(res.url);
    bytes = Buffer.from(await res.arrayBuffer());
  } catch (err) {
    if (err instanceof ProvisionError) throw err;
    throw networkError(err, urls.archive);
  }

  const actual = createHash('sha256').update(bytes).digest('hex');
  if (actual !== expected) {
    deps.log(`somme SHA256 invalide pour ${archiveName} — attendu ${expected}, obtenu ${actual}`);
    throw new ProvisionError(
      'VNL-EXT-002',
      `VNL-EXT-002: somme SHA256 invalide pour « ${archiveName} » (attendu ${expected}, obtenu ${actual}).`,
    );
  }

  // 7. extraction en tmpdir (le tmp est nettoyé dans tous les cas, finally).
  let dir: string;
  try {
    dir = await mkdtemp(join(deps.tmpdir(), 'vanyline-dl-'));
    await writeFile(join(dir, archiveName), bytes);
  } catch (err) {
    throw writeError(err, deps.tmpdir());
  }
  try {
    try {
      await deps.execFile('tar', ['-xf', join(dir, archiveName), '-C', dir]);
    } catch (err) {
      // Archive intégralement téléchargée mais illisible par tar : -002
      // (contenu incohérent avec son hash implicite — famille du cas ci-dessous).
      const reason = err instanceof Error ? err.message : String(err);
      throw new ProvisionError(
        'VNL-EXT-002',
        `VNL-EXT-002: extraction de l’archive « ${archiveName} » impossible (${reason}).`,
      );
    }
    const extracted = join(dir, 'vanyline');
    let extractedOk = false;
    try {
      await stat(extracted);
      extractedOk = true;
    } catch {
      extractedOk = false;
    }
    if (!extractedOk) {
      // Archive cohérente avec son hash mais contenu inattendu — code le plus
      // proche, écart documenté (contrat 03b).
      throw new ProvisionError(
        'VNL-EXT-002',
        'VNL-EXT-002: archive extraite sans binaire « vanyline » (contenu inattendu).',
      );
    }

    // 8. install atomique : chmod +x dans le tmp, copy vers un temp du même fs
    //    (binDir), rename final — jamais d'EXDEV, jamais d'exécution avant vérif.
    try {
      await chmod(extracted, 0o755);
      const tmpBin = join(
        binDir,
        `.vanyline.new-${process.pid}-${Math.random().toString(36).slice(2, 10)}`,
      );
      await copyFile(extracted, tmpBin);
      await rename(tmpBin, binPath);
    } catch (err) {
      throw writeError(err, binDir);
    }
  } finally {
    await rm(dir, { recursive: true, force: true });
  }

  // 9.
  deps.log(`vanyline ${EXPECTED_CLI_VERSION} installé : ${binPath}`);
  return { bin: binPath, source: 'installed' };
}

/** Wrapper promis de node:child_process.execFile — shell jamais utilisé (argv tableau strict). */
export function makeExecFile(): ProvisionDeps['execFile'] {
  return (cmd, args) =>
    new Promise((resolve, reject) => {
      nodeExecFile(cmd, [...args], { shell: false }, (err, stdout) => {
        if (err) {
          reject(err);
          return;
        }
        resolve({ stdout: String(stdout) });
      });
    });
}

/** deps de production (fetch global, os.homedir, process.platform/arch…). */
export function productionDeps(log: (line: string) => void): ProvisionDeps {
  return {
    fetch: (input, init) => globalThis.fetch(input, init),
    execFile: makeExecFile(),
    homedir: osHomedir,
    tmpdir: osTmpdir,
    platform: process.platform,
    arch: process.arch,
    log,
  };
}
