import { beforeEach, describe, expect, it, vi } from 'vitest';
import { mount } from '@vue/test-utils';
import AccountScreen from './AccountScreen.vue';

beforeEach(() => {
  vi.restoreAllMocks();
});

describe('AccountScreen', () => {
  it('affiche email et k8s_owner_name quand présents', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    fetchSpy.mockResolvedValueOnce({
      ok: true,
      status: 200,
      headers: new Map([['content-type', 'application/json']]),
      json: () => Promise.resolve({ email: 'a@b.c', k8s_owner_name: 'owner-x' }),
    } as unknown as Response);

    const wrapper = mount(AccountScreen);

    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    expect(wrapper.text()).toContain('a@b.c');
    expect(wrapper.text()).toContain('owner-x');
    expect(fetchSpy).toHaveBeenCalledWith('/api/me', expect.any(Object));
  });

  it('affiche un libellé d\'absence quand k8s_owner_name est null', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    fetchSpy.mockResolvedValueOnce({
      ok: true,
      status: 200,
      headers: new Map([['content-type', 'application/json']]),
      json: () => Promise.resolve({ email: 'a@b.c', k8s_owner_name: null }),
    } as unknown as Response);

    const wrapper = mount(AccountScreen);

    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    expect(wrapper.text()).toContain('a@b.c');
    expect(wrapper.text()).toContain('pas encore provisionné');
  });

  it('affiche le message d\'erreur quand l\'API répond 401', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    fetchSpy.mockResolvedValueOnce({
      ok: false,
      status: 401,
      headers: new Map([['content-type', 'application/json']]),
      json: () => Promise.resolve({ error: 'VNL-AUTH-001: Non autorisé' }),
    } as unknown as Response);

    const wrapper = mount(AccountScreen);

    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    expect(wrapper.text()).toContain('VNL-AUTH-001');
    expect(wrapper.text()).not.toContain('a@b.c');
  });
});