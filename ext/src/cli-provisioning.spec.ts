import { describe, expect, it } from 'vitest';
import {
  EXPECTED_CLI_VERSION,
  ProvisionError,
  isExpectedVersion,
  parseSha256File,
  releaseUrls,
  resolveTarget,
  validateFinalUrl,
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
