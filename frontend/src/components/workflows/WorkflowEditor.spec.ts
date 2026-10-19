import { mount } from '@vue/test-utils';
import { afterAll, afterEach, beforeAll, describe, expect, it } from 'vitest';
import type { Workflow, WorkflowCreate, WorkflowUpdate } from '@vanyline/protocol';
import { ApiError } from '../../api/client';
import WorkflowEditor from './WorkflowEditor.vue';

function workflow(partial: Partial<Workflow> & Pick<Workflow, 'scope' | 'steps'>): Workflow {
  return {
    id: 3,
    name: 'ci',
    description: 'intégration',
    kinds: [],
    steps_count: partial.steps.length,
    ...partial,
  };
}

// Montage rattaché au document : le submit d'un <form> n'est émis que dans un document (jsdom).
const montes: ReturnType<typeof mount>[] = [];
function monter(options: { props: Record<string, unknown> }) {
  const wrapper = mount(WorkflowEditor, { ...options, attachTo: document.body });
  montes.push(wrapper);
  return wrapper;
}

afterEach(() => {
  montes.splice(0).forEach((w) => w.unmount());
});

// jsdom n'implémente pas getClientRects, requis par CodeMirror pour le layout (cf. Editor.spec.ts).
const textProto = Text.prototype as unknown as Record<string, unknown>;
let gcSauve: unknown;
let gbSauve: unknown;
beforeAll(() => {
  gcSauve = textProto.getClientRects;
  gbSauve = textProto.getBoundingClientRect;
  textProto.getClientRects = () =>
    ({ item: () => null, length: 0, [Symbol.iterator]: function* () {} }) as unknown as DOMRectList;
  textProto.getBoundingClientRect = () =>
    ({ top: 0, left: 0, width: 0, height: 0, bottom: 0, right: 0, x: 0, y: 0 }) as DOMRect;
});
afterAll(() => {
  textProto.getClientRects = gcSauve;
  textProto.getBoundingClientRect = gbSauve;
});

function optionValues(wrapper: ReturnType<typeof monter>, testid: string): string[] {
  return wrapper
    .get(`[data-testid="${testid}"]`)
    .findAll('option')
    .map((o) => (o.element as HTMLOptionElement).value);
}

function savedBody(wrapper: ReturnType<typeof monter>): WorkflowCreate | WorkflowUpdate {
  const events = wrapper.emitted('save');
  expect(events, 'aucun événement save émis').toBeTruthy();
  return events![0][0] as WorkflowCreate | WorkflowUpdate;
}

describe('WorkflowEditor', () => {
  it('kinds_proposes_selon_la_portee', async () => {
    const projet = monter({ props: { workflow: null, defaultScope: 'project' } });
    await projet.get('[data-testid="add-step"]').trigger('click');
    expect(optionValues(projet, 'step-kind-0')).toEqual(['agent', 'rhai']);

    const sandbox = monter({ props: { workflow: null, defaultScope: 'sandbox' } });
    await sandbox.get('[data-testid="add-step"]').trigger('click');
    expect(optionValues(sandbox, 'step-kind-0')).toEqual(['agent', 'rhai', 'shell', 'gate']);
  });

  it('erreur_de_validation_affichee', async () => {
    const wrapper = monter({
      props: {
        workflow: workflow({
          scope: 'sandbox',
          steps: [{ id: 'build', kind: 'shell', depends_on: [], config: { script: 'make' } }],
        }),
      },
    });
    await wrapper.get('[data-testid="save"]').trigger('click');
    await wrapper.setProps({
      error: new ApiError(422, undefined, "VNL-WFA-005: step `build` : kind shell interdit pour la portée project"),
    });

    const step = wrapper.get('[data-testid="step"]');
    expect(step.get('[data-testid="step-error"]').text()).toContain('kind shell interdit');
  });

  it('seuil_de_couverture_desactive', async () => {
    const wrapper = monter({
      props: {
        workflow: workflow({
          scope: 'sandbox',
          steps: [{ id: 'qualite', kind: 'gate', depends_on: [], config: { min_coverage: 80 } }],
        }),
      },
    });
    await wrapper.get('[data-testid="gate-min-coverage-0"]').setValue('');
    await wrapper.get('[data-testid="save"]').trigger('click');

    const body = savedBody(wrapper) as WorkflowUpdate;
    const config = body.steps![0].config as { min_coverage?: number | null };
    expect(config.min_coverage ?? null).toBeNull();
  });

  it('mode_source', async () => {
    const steps = [{ id: 'a', kind: 'rhai' as const, depends_on: [], config: { script: 'let x = 1;' } }];
    const wrapper = monter({
      props: { workflow: workflow({ scope: 'sandbox', steps }) },
    });
    await wrapper.get('[data-testid="mode-source"]').trigger('click');

    const area = wrapper.get('[data-testid="source-json"]');
    expect(JSON.parse((area.element as HTMLTextAreaElement).value)).toEqual(steps);

    await area.setValue('{ invalide');
    await wrapper.get('[data-testid="save"]').trigger('click');
    expect(wrapper.emitted('save')).toBeFalsy();
    expect(wrapper.find('[data-testid="source-error"]').exists()).toBe(true);

    const corrige = [{ ...steps[0], id: 'b' }];
    await area.setValue(JSON.stringify(corrige));
    await wrapper.get('[data-testid="save"]').trigger('click');
    const body = savedBody(wrapper) as WorkflowUpdate;
    expect(body.steps).toEqual(corrige);
  });

  it('portee_figee_apres_creation', () => {
    const wrapper = monter({
      props: {
        workflow: workflow({
          scope: 'project',
          steps: [{ id: 'a', kind: 'agent', depends_on: [], config: { agent: 'x', prompt: 'y' } }],
        }),
      },
    });
    expect(wrapper.get('[data-testid="scope"]').attributes('disabled')).toBeDefined();
  });

  it('env_secret_masque_dans_l_editeur', async () => {
    const env = { API_TOKEN: 'tok-1', DB_PASSWORD: 'pwd-2', MY_SECRET: 'sec-3', SSH_KEY: 'key-4', LOG_LEVEL: 'debug' };
    const wrapper = monter({
      props: {
        workflow: workflow({
          scope: 'sandbox',
          steps: [{ id: 'build', kind: 'shell', depends_on: [], config: { script: 'make', env } }],
        }),
      },
    });

    // Then : les quatre clés « secret » sont masquées, LOG_LEVEL reste en clair.
    const valeurs = ['API_TOKEN', 'DB_PASSWORD', 'MY_SECRET', 'SSH_KEY', 'LOG_LEVEL'];
    valeurs.forEach((cle, r) => {
      const champ = wrapper.get(`[data-testid="env-value-0-${r}"]`);
      const attendu = cle === 'LOG_LEVEL' ? 'text' : 'password';
      expect(champ.attributes('type'), cle).toBe(attendu);
    });
    for (let r = 0; r < 4; r++) {
      expect(wrapper.get(`[data-testid="env-toggle-0-${r}"]`).text()).toBe('afficher');
    }
    expect(wrapper.find('[data-testid="env-toggle-0-4"]').exists()).toBe(false);

    // Le bouton « afficher » révèle la valeur de la ligne, sans toucher aux autres.
    await wrapper.get('[data-testid="env-toggle-0-0"]').trigger('click');
    const revele = wrapper.get('[data-testid="env-value-0-0"]');
    expect(revele.attributes('type')).toBe('text');
    expect((revele.element as HTMLInputElement).value).toBe('tok-1');
    expect(wrapper.get('[data-testid="env-toggle-0-0"]').text()).toBe('masquer');
    expect(wrapper.get('[data-testid="env-value-0-1"]').attributes('type')).toBe('password');

    // Non-régression : le masquage n'altère pas la valeur enregistrée.
    await wrapper.get('[data-testid="save"]').trigger('click');
    const body = savedBody(wrapper) as WorkflowUpdate;
    expect(body.steps![0].config).toEqual({ script: 'make', env });
  });
});
