// Contrat : ./shell.sdd (Scenario panneau_ferme_ne_rend_rien, panneau_ouvert_et_fermeture).
import { mount } from '@vue/test-utils';
import { describe, expect, it } from 'vitest';
import AssistantPanel from './AssistantPanel.vue';

describe('AssistantPanel — panneau_ferme_ne_rend_rien', () => {
  it('ne rend aucun complementary quand open=false', () => {
    const wrapper = mount(AssistantPanel, {
      props: { open: false, title: 'Assistant' },
      slots: { default: '<p>contenu</p>' },
    });
    expect(wrapper.find('[role="complementary"]').exists()).toBe(false);
    expect(wrapper.text()).not.toContain('contenu');
  });
});

describe('AssistantPanel — panneau_ouvert_et_fermeture', () => {
  it('rend un complementary nommé « Assistant » contenant le slot, puis émet close', async () => {
    const wrapper = mount(AssistantPanel, {
      props: { open: true, title: 'Assistant' },
      slots: { default: '<p>contenu du chat</p>' },
    });
    const aside = wrapper.get('[role="complementary"]');
    expect(aside.attributes('aria-label')).toBe('Assistant');
    expect(aside.text()).toContain('contenu du chat');

    await wrapper.get('button[aria-label="Fermer"]').trigger('click');
    expect(wrapper.emitted('close')).toHaveLength(1);
  });
});
