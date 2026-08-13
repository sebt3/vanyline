import { describe, expect, it } from 'vitest';
import { mount } from '@vue/test-utils';
import SettingsView from './SettingsView.vue';
import { activeNav } from './settings/navState';

describe('SettingsView', () => {
  it('affiche 5 groupes sans Projets ni Sandboxes', () => {
    const wrapper = mount(SettingsView);
    const items = wrapper.findAll('.nav-item');
    expect(items).toHaveLength(5);
    expect(wrapper.text()).toContain('Modèles');
    expect(wrapper.text()).toContain('Outils');
    expect(wrapper.text()).toContain('Agents');
    expect(wrapper.text()).toContain('Skills');
    expect(wrapper.text()).toContain('Compte');
    expect(wrapper.text()).not.toContain('Projets');
    expect(wrapper.text()).not.toContain('Sandboxes');
  });

  it('révèle 2 sous-items quand on clique sur "Modèles"', async () => {
    const wrapper = mount(SettingsView);
    const modelesItem = wrapper.find('[data-group="modeles"]');
    await modelesItem.trigger('click');
    await wrapper.vm.$nextTick();

    const subItems = wrapper.findAll('.nav-sub-item');
    expect(subItems).toHaveLength(2);
    expect(wrapper.text()).toContain('Fournisseurs LLM');
    expect(wrapper.text()).toContain('Profils de modèle');
  });

  it('révèle 2 sous-items quand on clique sur "Outils"', async () => {
    // D'abord cliquer sur un autre groupe pour reset
    const wrapper = mount(SettingsView);
    await wrapper.find('[data-group="agents"]').trigger('click');
    await wrapper.vm.$nextTick();

    // Maintenant cliquer Outils
    await wrapper.find('[data-group="outils"]').trigger('click');
    await wrapper.vm.$nextTick();

    const subItems = wrapper.findAll('.nav-sub-item');
    expect(subItems).toHaveLength(2);
    expect(wrapper.text()).toContain('Serveurs MCPs');
    expect(wrapper.text()).toContain('Toolsets');
  });

  it('monte les écrans Agents, Skills et Account', async () => {
    const wrapper = mount(SettingsView);

    const navItems = wrapper.findAll('.nav-item');

    // Agents → AgentsScreen
    await navItems[2].trigger('click');
    await wrapper.vm.$nextTick();
    expect(wrapper.findComponent({ name: 'AgentsScreen' }).exists()).toBe(true);

    // Skills → SkillsScreen
    await navItems[3].trigger('click');
    await wrapper.vm.$nextTick();
    expect(wrapper.findComponent({ name: 'SkillsScreen' }).exists()).toBe(true);

    // Compte → AccountScreen
    await navItems[4].trigger('click');
    await wrapper.vm.$nextTick();
    expect(wrapper.findComponent({ name: 'AccountScreen' }).exists()).toBe(true);
  });

  it('synchronise activeNav après clic sur Outils', async () => {
    const wrapper = mount(SettingsView);

    // Cliquer Outils
    await wrapper.find('[data-group="outils"]').trigger('click');
    await wrapper.vm.$nextTick();

    expect(activeNav.value).toEqual({
      groupLabel: 'Outils',
      screenLabel: 'Serveurs MCPs',
    });
  });
});
