import { describe, expect, it } from 'vitest';
import { mount } from '@vue/test-utils';
import SettingsView from './SettingsView.vue';

describe('SettingsView', () => {
  it('affiche les 4 groupes de navigation', () => {
    const wrapper = mount(SettingsView);
    const items = wrapper.findAll('.nav-item');
    expect(items).toHaveLength(4);
    expect(wrapper.text()).toContain('Projets');
    expect(wrapper.text()).toContain('Sandboxes');
    expect(wrapper.text()).toContain('Agent & modèle');
    expect(wrapper.text()).toContain('Compte');
  });

  it('révèle les 6 sous-items quand on clique sur "Agent & modèle"', async () => {
    const wrapper = mount(SettingsView);
    const agentItem = wrapper.find('[data-group="agent"]');
    await agentItem.trigger('click');
    await wrapper.vm.$nextTick();

    const subItems = wrapper.findAll('.nav-sub-item');
    expect(subItems).toHaveLength(6);
    expect(wrapper.text()).toContain('Fournisseurs LLM');
    expect(wrapper.text()).toContain('Profils de modèle');
    expect(wrapper.text()).toContain('Toolsets');
    expect(wrapper.text()).toContain('Skills');
    expect(wrapper.text()).toContain('Agents');
    expect(wrapper.text()).toContain('Serveurs MCP');
  });

  it('monte AccountScreen quand on sélectionne le groupe "account"', async () => {
    const wrapper = mount(SettingsView);
    // Par défaut, Projects → Pending
    expect(wrapper.text()).toContain('À venir');

    // Le 4ème bouton de nav (index 3) est "Compte"
    const navItems = wrapper.findAll('.nav-item');
    await navItems[3].trigger('click');
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    // L'API n'est pas mockée en test, mais AccountScreen doit être monté
    // (il affiche un message d'erreur réseau au lieu des champs)
    const rendered = wrapper.findComponent({ name: 'AccountScreen' });
    expect(rendered.exists()).toBe(true);
  });
});