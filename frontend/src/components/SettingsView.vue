<script setup lang="ts">
import { TabsRoot, TabsList, TabsTrigger, TabsContent } from 'reka-ui';

interface Field {
  label: string;
  type: 'text' | 'select';
  value: string;
  options?: string[];
  hint?: string;
  wide?: boolean;
}

interface Category {
  id: string;
  label: string;
  icon: string;
  accent: string;
  subtitle: string;
  fields: Field[];
}

const categories: Category[] = [
  {
    id: 'project',
    label: 'Projet',
    icon: '⌥',
    accent: '#4c90f0',
    subtitle: 'Le dépôt et la branche sur lesquels la sandbox va travailler.',
    fields: [
      { label: 'Dépôt git', type: 'text', value: 'git@git.kydah.fr:shuss/media-station.git', wide: true },
      { label: 'Branche', type: 'select', value: 'main', options: ['main', 'feat/thumbnails-webp'] },
    ],
  },
  {
    id: 'sandbox',
    label: 'Sandbox',
    icon: '▣',
    accent: '#3fb56d',
    subtitle: "L'environnement d'exécution provisionné pour ce projet.",
    fields: [
      { label: 'Toolchains', type: 'text', value: 'python, rust', hint: 'liste séparée par des virgules', wide: true },
      { label: 'CPU', type: 'select', value: '2 vCPU', options: ['1 vCPU', '2 vCPU', '4 vCPU'] },
      { label: 'Mémoire', type: 'select', value: '4 Gi', options: ['2 Gi', '4 Gi', '8 Gi'] },
    ],
  },
  {
    id: 'agent',
    label: 'Agent & modèle',
    icon: '✦',
    accent: '#5b1ecf',
    subtitle: 'Le LLM que consulte l\'assistant, et où il vit.',
    fields: [
      { label: 'Endpoint', type: 'text', value: 'http://ollama.kydah.svc.cluster.local:11434', wide: true },
      { label: 'Modèle', type: 'select', value: 'qwen3.6:35b-a3b', options: ['qwen3.6:35b-a3b', 'llama3.2:8b'] },
    ],
  },
  {
    id: 'account',
    label: 'Compte',
    icon: '●',
    accent: '#e0a83d',
    subtitle: 'Ton identité cluster et ce qui en dépend.',
    fields: [{ label: 'Owner', type: 'text', value: 'shuss' }],
  },
];
</script>

<template>
  <TabsRoot class="settings" default-value="project" orientation="vertical">
    <TabsList class="nav" aria-label="Catégories de configuration">
      <TabsTrigger
        v-for="cat in categories"
        :key="cat.id"
        class="nav-item"
        :value="cat.id"
        :style="{ '--accent': cat.accent }"
      >
        <span class="nav-icon">{{ cat.icon }}</span>
        {{ cat.label }}
      </TabsTrigger>
    </TabsList>
    <div class="panels">
      <TabsContent v-for="cat in categories" :key="cat.id" class="panel" :value="cat.id" :style="{ '--accent': cat.accent }">
        <div class="panel-head">
          <span class="panel-icon">{{ cat.icon }}</span>
          <div>
            <h2>{{ cat.label }}</h2>
            <p class="subtitle">{{ cat.subtitle }}</p>
          </div>
        </div>

        <div class="card">
          <label v-for="field in cat.fields" :key="field.label" class="field" :class="{ wide: field.wide }">
            <span class="field-label">{{ field.label }}</span>
            <select v-if="field.type === 'select'" :value="field.value">
              <option v-for="opt in field.options" :key="opt" :value="opt">{{ opt }}</option>
            </select>
            <input v-else type="text" :value="field.value" />
            <span v-if="field.hint" class="field-hint">{{ field.hint }}</span>
          </label>
        </div>

        <div v-if="cat.id === 'project'" class="action-row">
          <button class="primary-btn">Ouvrir le workspace</button>
          <span class="status-pill ok">● dépôt joignable</span>
        </div>
        <div v-else-if="cat.id === 'sandbox'" class="status-row">
          <span class="status-pill ok">● sandbox active</span>
          <span class="status-dim">provisionnée il y a 14j</span>
        </div>
        <div v-else-if="cat.id === 'agent'" class="status-row">
          <span class="status-pill ok">● endpoint joignable</span>
          <span class="status-dim">latence moyenne 180ms</span>
        </div>
        <div v-else-if="cat.id === 'account'" class="quota">
          <div class="quota-row">
            <span>Sandboxes utilisées</span>
            <span class="quota-value">3 / 5</span>
          </div>
          <div class="quota-bar"><div class="quota-fill" style="width: 60%" /></div>
        </div>
      </TabsContent>
    </div>
  </TabsRoot>
</template>

<style scoped>
.settings {
  height: 100%;
  width: 100%;
  display: flex;
  background: #0c1420;
  color: #e6e9f0;
  font-size: 13px;
}
.nav {
  width: 240px;
  flex: none;
  display: flex;
  flex-direction: column;
  gap: 3px;
  padding: 24px 14px;
  border-right: 1px solid #1c1c2a;
}
.nav-item {
  appearance: none;
  border: none;
  border-left: 3px solid transparent;
  background: transparent;
  color: #9497a9;
  display: flex;
  align-items: center;
  gap: 10px;
  text-align: left;
  font: inherit;
  font-size: 13.5px;
  height: 38px;
  padding: 0 10px 0 10px;
  border-radius: 0 6px 6px 0;
  cursor: default;
}
.nav-icon {
  width: 18px;
  text-align: center;
  font-size: 15px;
  color: var(--accent);
}
.nav-item:hover {
  background: #161d2c;
  color: white;
}
.nav-item[data-state='active'] {
  background: #161d2c;
  border-left-color: var(--accent);
  color: white;
  font-weight: 600;
}

.panels {
  flex: 1;
  overflow-y: auto;
  padding: 48px 56px;
}
.panel-head {
  display: flex;
  align-items: flex-start;
  gap: 18px;
  margin-bottom: 32px;
  max-width: 760px;
}
.panel-icon {
  width: 48px;
  height: 48px;
  flex: none;
  display: flex;
  align-items: center;
  justify-content: center;
  border-radius: 10px;
  background: color-mix(in srgb, var(--accent) 18%, transparent);
  color: var(--accent);
  font-size: 21px;
}
.panel h2 {
  margin: 3px 0 6px;
  font-size: 24px;
  font-weight: 700;
}
.subtitle {
  margin: 0;
  color: #9497a9;
  font-size: 13.5px;
}

.card {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 22px 28px;
  max-width: 760px;
  padding: 28px 32px;
  background: #101828;
  border: 1px solid #1c1c2a;
  border-top: 2px solid var(--accent);
  border-radius: 10px;
}
.field {
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.field.wide {
  grid-column: 1 / -1;
}
.field-label {
  font-size: 11px;
  color: #9497a9;
  text-transform: uppercase;
  letter-spacing: 0.05em;
}
.field input,
.field select {
  height: 30px;
  padding: 0 9px;
  background: #0c1420;
  border: 1px solid #2b2b4a;
  border-radius: 4px;
  color: #e6e9f0;
  font: inherit;
  font-family: ui-monospace, "SFMono-Regular", Menlo, Consolas, monospace;
  font-size: 12.5px;
}
.field input:focus,
.field select:focus {
  outline: none;
  border-color: var(--accent);
}
.field-hint {
  font-size: 11px;
  color: #6a7185;
}

.action-row,
.status-row {
  display: flex;
  align-items: center;
  gap: 14px;
  margin-top: 24px;
  max-width: 760px;
}
.primary-btn {
  appearance: none;
  border: none;
  height: 32px;
  padding: 0 16px;
  border-radius: 5px;
  background: var(--accent);
  color: white;
  font: inherit;
  font-weight: 600;
  font-size: 12.5px;
  cursor: default;
}
.status-pill {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  font-size: 11.5px;
  padding: 4px 10px;
  border-radius: 999px;
}
.status-pill.ok {
  background: rgba(63, 181, 109, 0.15);
  color: #3fb56d;
}
.status-dim {
  font-size: 11.5px;
  color: #6a7185;
}

.quota {
  max-width: 760px;
  margin-top: 8px;
}
.quota-row {
  display: flex;
  justify-content: space-between;
  font-size: 12px;
  color: #9497a9;
  margin-bottom: 6px;
}
.quota-value {
  color: #e6e9f0;
  font-family: ui-monospace, "SFMono-Regular", Menlo, Consolas, monospace;
}
.quota-bar {
  height: 6px;
  border-radius: 999px;
  background: #1c1c2a;
  overflow: hidden;
}
.quota-fill {
  height: 100%;
  background: var(--accent);
  border-radius: 999px;
}
</style>
