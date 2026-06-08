<script lang="ts">
  let {
    disabled = false,
    onSend,
  }: {
    disabled?: boolean;
    onSend: (content: string) => void;
  } = $props();

  let value = $state('');

  function send() {
    const text = value.trim();
    if (!text || disabled) return;
    onSend(text);
    value = '';
  }

  function onKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      send();
    }
  }
</script>

<div class="flex gap-2 p-3 border-t border-stone-800">
  <textarea
    bind:value
    onkeydown={onKeydown}
    {disabled}
    rows="1"
    placeholder="Message… (Enter to send, Shift+Enter for newline)"
    class="flex-1 resize-none rounded bg-stone-800 border border-stone-700 text-stone-100 text-sm px-3 py-2 placeholder-stone-600 focus:outline-none focus:border-primary-600 disabled:opacity-50"
  ></textarea>
  <button
    onclick={send}
    {disabled}
    class="rounded bg-primary-600 hover:bg-primary-500 disabled:opacity-40 disabled:cursor-not-allowed px-4 py-2 text-white text-sm font-medium"
  >Send</button>
</div>
