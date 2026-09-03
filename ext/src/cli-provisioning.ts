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
