import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { debounce, layoutStorageKey, loadLayout, saveLayout } from './ideLayoutPersistence';

describe('layoutStorageKey', () => {
  it('une clé par sandbox', () => {
    expect(layoutStorageKey('foo')).toBe('vanyline.ide.layout.foo');
    expect(layoutStorageKey('bar')).toBe('vanyline.ide.layout.bar');
  });
});

describe('loadLayout / saveLayout', () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it("aucune sauvegarde → null", () => {
    expect(loadLayout('foo')).toBeNull();
  });

  it('round-trip save puis load', () => {
    const layout = { grid: { root: {} } } as unknown as Parameters<typeof saveLayout>[1];
    saveLayout('foo', layout);
    expect(loadLayout('foo')).toEqual(layout);
  });

  it('JSON corrompu → null, pas de throw', () => {
    localStorage.setItem(layoutStorageKey('foo'), 'not json{{{');
    expect(loadLayout('foo')).toBeNull();
  });

  it('deux sandboxes ne se marchent pas dessus', () => {
    saveLayout('foo', { a: 1 } as unknown as Parameters<typeof saveLayout>[1]);
    saveLayout('bar', { b: 2 } as unknown as Parameters<typeof saveLayout>[1]);
    expect(loadLayout('foo')).toEqual({ a: 1 });
    expect(loadLayout('bar')).toEqual({ b: 2 });
  });
});

describe('debounce', () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it('regroupe les appels rapprochés en un seul', () => {
    const fn = vi.fn();
    const debounced = debounce(fn, 400);

    debounced();
    debounced();
    debounced();
    expect(fn).not.toHaveBeenCalled();

    vi.advanceTimersByTime(400);
    expect(fn).toHaveBeenCalledTimes(1);
  });

  it('transmet les arguments du dernier appel', () => {
    const fn = vi.fn();
    const debounced = debounce((x: number) => fn(x), 400);

    debounced(1);
    debounced(2);
    vi.advanceTimersByTime(400);

    expect(fn).toHaveBeenCalledWith(2);
  });
});
