import { describe, it, expect } from 'vitest';
import { iconForPath, folderIcon, genericFileIcon } from './fileIcon';
import { Cpu, DataLine, Notebook, SetUp, Document, MagicStick, Goods, Reading,
  Film } from '@element-plus/icons-vue';

describe('fileIcon.ts — mapping extension → icône', () => {
  it('iconForPath déduit l\'icône de l\'extension', () => {
    expect(iconForPath('src/main.ts')).toBe(Cpu);
    expect(iconForPath('a.json')).toBe(DataLine);
    expect(iconForPath('README.md')).toBe(Notebook);
    expect(iconForPath('b.yaml')).toBe(SetUp);
    expect(iconForPath('App.vue')).toBe(MagicStick);
    expect(iconForPath('engine.rhai')).toBe(Reading);
    expect(iconForPath('views/page.hbs')).toBe(Film);
    expect(iconForPath('views/page.handlebars')).toBe(Film);
  });

  it('Dockerfile reconnu par nom de base, pas par extension', () => {
    expect(iconForPath('Dockerfile')).toBe(Goods);
    expect(iconForPath('deploy/Dockerfile.dev')).toBe(Goods);
    expect(iconForPath('app.dockerfile')).toBe(Goods);
    expect(iconForPath('Containerfile')).toBe(Goods);
  });

  it('extension inconnue ou null → icône générique', () => {
    expect(iconForPath('Makefile')).toBe(genericFileIcon);
    expect(iconForPath('a.xyz')).toBe(genericFileIcon);
    expect(iconForPath(null)).toBe(genericFileIcon);
  });

  it('folderIcon et genericFileIcon sont définis', () => {
    expect(folderIcon).toBeDefined();
    expect(typeof folderIcon).toBe('object');
    expect(genericFileIcon).toBe(Document);
  });
});
