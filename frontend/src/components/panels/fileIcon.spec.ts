import { describe, it, expect } from 'vitest';
import { iconForPath, folderIcon, genericFileIcon } from './fileIcon';
import { Cpu, DataLine, Notebook, SetUp, Document } from '@element-plus/icons-vue';

describe('fileIcon.ts — mapping extension → icône', () => {
  it('iconForPath déduit l\'icône de l\'extension', () => {
    expect(iconForPath('src/main.ts')).toBe(Cpu);
    expect(iconForPath('a.json')).toBe(DataLine);
    expect(iconForPath('README.md')).toBe(Notebook);
    expect(iconForPath('b.yaml')).toBe(SetUp);
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
