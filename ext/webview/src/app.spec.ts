// @vitest-environment jsdom
import { describe, expect, it } from 'vitest';
import { flushPromises, mount } from '@vue/test-utils';
import App from './App.vue';

describe('App (spike webview)', () => {
  it('monte ChatWindow (chat-host) avec les ports stub', async () => {
    const wrapper = mount(App);
    // ChatWindow appelle listConversations() au mount → le stub résout [] sans erreur.
    await flushPromises();
    expect(wrapper.find('.chat-host').exists()).toBe(true);
  });
});
