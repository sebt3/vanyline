import * as esbuild from 'esbuild';

// console.log autorisé dans les scripts de build (la règle anti-console vise src/).
const target = process.argv[2];
if (target !== 'extension') {
  console.error('Usage: node esbuild.mjs extension');
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
});
console.log('Built extension');
