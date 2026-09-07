import { describe, expect, it } from 'vitest';
import { dirRootUri, languageExtensionForPath, lspToolchainForPath } from './editorLanguage';

describe('languageExtensionForPath', () => {
  it.each([
    ['a.ts', 'ts'],
    ['a.tsx', 'tsx'],
    ['a.js', 'js'],
    ['a.jsx', 'jsx'],
    ['a.rs', 'rs'],
    ['a.json', 'json'],
    ['README.md', 'md'],
    ['a.toml', 'toml'],
    ['a.yaml', 'yaml'],
    ['a.yml', 'yml'],
    ['a.py', 'py'],
    ['a.vue', 'vue'],
    ['a.rhai', 'rhai'],
    ['a.hbs', 'hbs'],
    ['a.handlebars', 'handlebars'],
  ])('renvoie une extension non vide pour %s', (path) => {
    expect(languageExtensionForPath(path).length).toBeGreaterThan(0);
  });

  it('.pyi → coloration python des stubs (tableau de longueur 1)', () => {
    expect(languageExtensionForPath('stubs/foo.pyi')).toHaveLength(1);
  });

  it('chemin sans extension reconnue → tableau vide', () => {
    expect(languageExtensionForPath('Makefile')).toEqual([]);
    // `.pyc` (bytecode compilé) reste hors service — miroir des négatifs
    // `.pyc`/`.pyx` du mapping LSP (task-04).
    expect(languageExtensionForPath('x.pyc')).toHaveLength(0);
  });

  it('nom de base Dockerfile → mode dockerfile (tableau de longueur 1)', () => {
    expect(languageExtensionForPath('Dockerfile')).toHaveLength(1);
    expect(languageExtensionForPath('deploy/Dockerfile.dev')).toHaveLength(1);
    expect(languageExtensionForPath('app.dockerfile')).toHaveLength(1);
  });

  it('Dockerfile reconnu alors que le chemin ne contient aucun point', () => {
    // `path.split('.').pop()` seul ne suffirait pas : 'Dockerfile' n'a pas
    // d'extension, c'est un nom de base.
    expect('Dockerfile').not.toContain('.');
    expect(languageExtensionForPath('Dockerfile')).not.toEqual([]);
  });

  it('null → tableau vide', () => {
    expect(languageExtensionForPath(null)).toEqual([]);
  });

  it('.vue avec chemin à points multiples', () => {
    // L'extension retenue est bien `vue` (dernier segment), pas `bar`.
    expect(languageExtensionForPath('src/components/Foo.bar.vue')).toHaveLength(1);
  });

  it("l'extension est insensible à la casse", () => {
    expect(languageExtensionForPath('a.TS').length).toBeGreaterThan(0);
    expect(languageExtensionForPath('App.VUE')).toHaveLength(1);
  });
});

describe('lspToolchainForPath', () => {
  it.each([
    ['src/main.rs', { toolchain: 'rust', languageId: 'rust' }],
    ['a.ts', { toolchain: 'node', languageId: 'typescript' }],
    ['a.tsx', { toolchain: 'node', languageId: 'typescript' }],
    ['a.mts', { toolchain: 'node', languageId: 'typescript' }],
    ['a.cts', { toolchain: 'node', languageId: 'typescript' }],
    ['a.js', { toolchain: 'node', languageId: 'javascript' }],
    ['a.jsx', { toolchain: 'node', languageId: 'javascript' }],
    ['a.mjs', { toolchain: 'node', languageId: 'javascript' }],
    ['a.cjs', { toolchain: 'node', languageId: 'javascript' }],
    ['App.vue', { toolchain: 'node', languageId: 'vue' }],
    // py/pyi mappés python (miroir sandbox `toolchain_for_path`).
    ['services/api/main.py', { toolchain: 'python', languageId: 'python' }],
    ['stubs/foo.pyi', { toolchain: 'python', languageId: 'python' }],
    ['Dockerfile', { toolchain: 'docker', languageId: 'dockerfile' }],
    ['deploy/Dockerfile', { toolchain: 'docker', languageId: 'dockerfile' }],
    ['Dockerfile.dev', { toolchain: 'docker', languageId: 'dockerfile' }],
    ['build/app.dockerfile', { toolchain: 'docker', languageId: 'dockerfile' }],
    ['Containerfile', { toolchain: 'docker', languageId: 'dockerfile' }],
    // Nom de base gagnant sur l'extension (miroir sandbox) : `Dockerfile.ts`
    // est docker/dockerfile, pas node/typescript.
    ['Dockerfile.ts', { toolchain: 'docker', languageId: 'dockerfile' }],
  ])('%s → %s', (path, expected) => {
    expect(lspToolchainForPath(path)).toEqual(expected);
  });

  // `.py` est mappé python depuis task-04 — retiré des négatifs (la règle
  // docker reste évaluée avant, verrouillée par le test de précédence).
  it.each(['a.rhai', 'a.hbs', null])('retourne null pour %s', (path) => {
    expect(lspToolchainForPath(path)).toBeNull();
  });

  it('l\'extension est insensible à la casse', () => {
    expect(lspToolchainForPath('A.RS')).toEqual({ toolchain: 'rust', languageId: 'rust' });
    expect(lspToolchainForPath('A.TS')).toEqual({ toolchain: 'node', languageId: 'typescript' });
    expect(lspToolchainForPath('A.JS')).toEqual({ toolchain: 'node', languageId: 'javascript' });
    expect(lspToolchainForPath('App.VUE')).toEqual({ toolchain: 'node', languageId: 'vue' });
    expect(lspToolchainForPath('DOCKERFILE')).toEqual({ toolchain: 'docker', languageId: 'dockerfile' });
    expect(lspToolchainForPath('MAIN.PY')).toEqual({ toolchain: 'python', languageId: 'python' });
  });

  it('précédence du nom de base docker sur .py ; .pyc/.pyx hors service', () => {
    // Miroir sandbox : `Dockerfile.py` est docker/dockerfile, pas python (le
    // nom de base passe AVANT le switch d'extension — patron
    // `toolchain_for_path_python_precedence_dockerfile` côté rust).
    expect(lspToolchainForPath('Dockerfile.py')).toEqual({ toolchain: 'docker', languageId: 'dockerfile' });
    // Suffixes voisins, mêmes négatifs que la détection tâche 01.
    expect(lspToolchainForPath('data.pyc')).toBeNull();
    expect(lspToolchainForPath('kernel.pyx')).toBeNull();
  });
});

describe('dirRootUri', () => {
  it('fichier niché : répertoire contenant le fichier', () => {
    expect(dirRootUri('frontend/src/components/panels/Editor.vue'))
      .toBe('file:///frontend/src/components/panels');
  });

  it('fichier directement sous un répertoire : ce répertoire', () => {
    expect(dirRootUri('frontend/App.vue')).toBe('file:///frontend');
  });

  it('fichier à la racine (pas de /) : racine du workspace', () => {
    expect(dirRootUri('Cargo.toml')).toBe('file:///');
  });
});
