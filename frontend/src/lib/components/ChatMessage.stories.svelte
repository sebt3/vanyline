<script module>
  export const meta = {
    title: 'Components/ChatMessage',
    component: ChatMessage,
  };
</script>

<script lang="ts">
  import { Story, Template } from '@storybook/addon-svelte-csf';
  import ChatMessage from './ChatMessage.svelte';
</script>

<Template let:args>
  <div class="w-full max-w-2xl bg-stone-950 py-2">
    <ChatMessage {...args} />
  </div>
</Template>

<Story name="User" args={{ message: { role: 'user', content: 'What is Kubernetes?' } }} />
<Story name="Assistant" args={{ message: { role: 'assistant', content: 'Kubernetes is a container orchestration platform...' } }} />
<Story name="Streaming" args={{ message: { role: 'assistant', content: 'Let me think about', streaming: true } }} />
<Story name="Tool call" args={{ message: { role: 'assistant', content: 'Running the command...', tool_calls: [{ name: 'bash', args: { command: 'kubectl get pods' } }] } }} />
<Story name="Long" args={{ message: { role: 'assistant', content: 'This is a much longer response that goes into detail about how Kubernetes schedules workloads across nodes using the scheduler component, which evaluates resource requests, node affinity rules, taints, and tolerations to find the optimal placement.' } }} />
