import { afterEach, describe, expect, it, vi } from 'vitest';
import { mount } from '@vue/test-utils';
import GlobalSettingsScreen from './GlobalSettingsScreen.vue';

function jsonRes(body: unknown) {
  return {
    ok: true,
    status: 200,
    headers: new Map([['content-type', 'application/json']]),
    json: () => Promise.resolve(body),
  } as unknown as Response;
}

async function flush() {
  for (let i = 0; i < 6; i++) await new Promise((r) => setTimeout(r, 0));
}

/** Routes par clé `MÉTHODE url`, 500 sinon. */
function routes(spy: ReturnType<typeof vi.spyOn>, table: Record<string, unknown>) {
  (spy as any).mockImplementation(async (url: string, init?: RequestInit) => {
    const key = `${init?.method ?? 'GET'} ${String(url)}`;
    if (key in table) return jsonRes(table[key]);
    return new Response(null, { status: 500 });
  });
}

function bodyOf(spy: { mock: { calls: unknown[][] } }, method: string, url: string) {
  const call = spy.mock.calls.find(
    (c) => c[0] === url && ((c[1] as RequestInit | undefined)?.method ?? 'GET') === method,
  );
  return call ? JSON.parse((call[1] as RequestInit).body as string) : undefined;
}

async function fill(wrapper: ReturnType<typeof mount>, provider: string, model: string, options: string) {
  await wrapper.find('select[aria-label="Fournisseur global"]').setValue(provider);
  await wrapper.find('input[aria-label="Nom du modèle"]').setValue(model);
  await wrapper.find('textarea[aria-label="Options (JSON)"]').setValue(options);
}

const PROVIDERS = [{ id: 1, name: 'ollama-local' }];

describe('GlobalSettingsScreen — SettingsView.sdd (G2a)', () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('modele_par_defaut', async () => {
    const spy = vi.spyOn(globalThis, 'fetch');
    routes(spy, {
      'GET /api/v1/llm-providers': PROVIDERS,
      'GET /api/v1/app-settings': [],
      'POST /api/v1/app-settings': { id: 7, key: 'default_model', value: {} },
    });
    const wrapper = mount(GlobalSettingsScreen);
    await flush();
    await fill(wrapper, '1', 'qwen3.6:35b-a3b', '{"temperature":0.2}');
    await wrapper.find('button').trigger('click');
    await flush();
    expect(bodyOf(spy, 'POST', '/api/v1/app-settings')).toEqual({
      key: 'default_model',
      value: { provider_id: 1, model: 'qwen3.6:35b-a3b', options: { temperature: 0.2 } },
    });
  });

  it('modele_par_defaut_mise_a_jour', async () => {
    const spy = vi.spyOn(globalThis, 'fetch');
    routes(spy, {
      'GET /api/v1/llm-providers': PROVIDERS,
      'GET /api/v1/app-settings': [
        { id: 7, key: 'default_model', value: { provider_id: 1, model: 'ancien' } },
      ],
      'PUT /api/v1/app-settings/7': { id: 7, key: 'default_model', value: {} },
    });
    const wrapper = mount(GlobalSettingsScreen);
    await flush();
    expect((wrapper.find('input[aria-label="Nom du modèle"]').element as HTMLInputElement).value).toBe('ancien');
    await fill(wrapper, '1', 'nouveau', '');
    await wrapper.find('button').trigger('click');
    await flush();
    expect(bodyOf(spy, 'PUT', '/api/v1/app-settings/7')).toEqual({
      key: 'default_model',
      value: { provider_id: 1, model: 'nouveau' },
    });
  });

  it('options_json_invalide_non_envoyees', async () => {
    const spy = vi.spyOn(globalThis, 'fetch');
    routes(spy, { 'GET /api/v1/llm-providers': PROVIDERS, 'GET /api/v1/app-settings': [] });
    const wrapper = mount(GlobalSettingsScreen);
    await flush();
    await fill(wrapper, '1', 'm', '{pas du json');
    await wrapper.find('button').trigger('click');
    await flush();
    expect(bodyOf(spy, 'POST', '/api/v1/app-settings')).toBeUndefined();
    expect(wrapper.text()).toContain('JSON');
  });
});
