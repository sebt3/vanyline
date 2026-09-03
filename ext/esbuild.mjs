import * as esbuild from 'esbuild';
import * as fs from 'node:fs';

// console.log autorisé dans les scripts de build (la règle anti-console vise src/).
const target = process.argv[2];
if (target !== 'extension') {
  console.error('Usage: node esbuild.mjs extension');
  process.exit(1);
}

// Version CLI bakée au build (cf. docs/features/F3 — provisioning).
const cliVersion = fs.readFileSync(new URL('./cli-version.txt', import.meta.url), 'utf8').trim();
if (!/^\d+\.\d+\.\d+([-+].+)?$/.test(cliVersion)) {
  console.error(`cli-version.txt invalide: «${cliVersion}»`);
  process.exit(1);
}

await esbuild.build({
  bundle: true,
  format: 'cjs',
  platform: 'node',
  target: 'node20',
  sourcemap: true,
  entryPoints: ['src/extension.ts'],
  outfile: 'dist/extension/index.js',
  external: ['vscode'],
  define: { __EXPECTED_CLI_VERSION__: JSON.stringify(cliVersion) },
});
console.log('Built extension');
