<script module>
  export const meta = {
    title: 'Components/ChatInput',
    component: ChatInput,
  };
</script>

<script lang="ts">
  import { Story, Template } from '@storybook/addon-svelte-csf';
  import ChatInput from './ChatInput.svelte';
</script>

<Template let:args>
  <div class="w-full max-w-2xl bg-stone-950">
    <ChatInput {...args} onSend={() => {}} />
  </div>
</Template>

<Story name="Actif" args={{ disabled: false }} />
<Story name="Désactivé (streaming)" args={{ disabled: true }} />
