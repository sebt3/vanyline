<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from 'vue';
import { useRouter } from 'vue-router';
import { DialogClose } from 'reka-ui';
import { ApiError, createApiClient } from '../../api/client';
import { useCrudResource } from '../../composables/useCrudResource';
import {
  useSandboxState,
  reconnectNow,
  registerProjectRefresh,
  unregisterProjectRefresh,
  registerRefresh,
  unregisterRefresh,
} from '../../composables/useSandboxState';
import LoadingSkeleton from '../common/LoadingSkeleton.vue';
import ErrorCard from '../common/ErrorCard.vue';
import EmptyState from '../common/EmptyState.vue';
import DialogShell from '../common/DialogShell.vue';
import Field from '../common/Field.vue';
import ContextChat from '../ContextChat.vue';
import AssistantPanel from '../shell/AssistantPanel.vue';
import { MD_MIN_WIDTH, useViewportWidth } from './viewport';

interface ProjectSpec {
  owner: string;
  repoUrl: string;
  defaultBranch?: string | null;
}

interface Project {
  metadata: { name: string };
  spec: ProjectSpec;
  status?: { cloned?: boolean } | null;
}

interface Sandbox {
  metadata: { name: string };
  spec: { project: string };
  status?: { phase?: string | null } | null;
}

interface CreateProjectBody {
  name: string;
  repoUrl: string;
  defaultBranch?: string;
}

const client = createApiClient();
const router = useRouter();
const resource = useCrudResource<Project>(client, '/api/projects');
const { items: fetchedProjects, loading, error } = resource;
/** Sandboxes de tous les projets : seulement pour les pastilles des cartes. */
const sandboxes = useCrudResource<Sandbox>(client, '/api/sandboxes');

function openProject(name: string) {
  router.push(`/p/${encodeURIComponent(name)}`);
}

const formName = ref('');
const formRepo = ref('');
const formBranch = ref('');
const creationError = ref<string | null>(null);

const modalOpen = ref(false);

/** Assistant d'accueil (ContextChat, contexte `home`) : panneau commun (shell.sdd). */
const width = useViewportWidth();
const stacked = computed(() => width.value < MD_MIN_WIDTH);
const assistantOpen = ref(false);
const homeChatData = {};

/** État du clone initial : tiré du statut du Project, sans recalcul côté client. */
function cloneLabel(p: Project): string {
  if (p.status?.cloned === true) return 'Cloné';
  if (p.status?.cloned === false) return 'En cours';
  return '—';
}

function isCloning(p: Project): boolean {
  return p.status?.cloned === false;
}

function stateTone(p: Project): 'ok' | 'warn' | 'muted' {
  if (p.status?.cloned === true) return 'ok';
  if (p.status?.cloned === false) return 'warn';
  return 'muted';
}

/** Pastilles d'une carte : décompte des sandboxes du projet par phase (« Running » = active). */
function sandboxPills(name: string): { label: string; active: boolean }[] {
  if (sandboxes.error.value) return [];
  const counts = new Map<string, number>();
  for (const s of sandboxes.items.value) {
    if (s.spec.project !== name) continue;
    const phase = s.status?.phase ?? 'inconnue';
    counts.set(phase, (counts.get(phase) ?? 0) + 1);
  }
  if (counts.size === 0) return [{ label: 'aucune sandbox', active: false }];
  return [...counts].map(([phase, n]) =>
    phase === 'Running'
      ? { label: `${n} active${n > 1 ? 's' : ''}`, active: true }
      : { label: `${n} ${phase}`, active: false },
  );
}

// Hub temps réel : la liste se relit sur tout événement `project`, les sandboxes sur
// tout événement `sandbox` (pastilles des cartes).
useSandboxState();
onMounted(() => {
  resource.fetch();
  sandboxes.fetch();
  registerProjectRefresh(resource.fetch);
  registerRefresh(sandboxes.fetch);
});
onBeforeUnmount(() => {
  unregisterProjectRefresh(resource.fetch);
  unregisterRefresh(sandboxes.fetch);
});

async function createProject() {
  creationError.value = null;
  const body: CreateProjectBody = {
    name: formName.value,
    repoUrl: formRepo.value,
    defaultBranch: formBranch.value || undefined,
  };
  try {
    await resource.create(body);
    // Un utilisateur sans Owner voit son socket fermé aussitôt par le backend :
    // on rouvre sans attendre le back-off pour s'abonner au premier Project.
    reconnectNow();
    formName.value = '';
    formRepo.value = '';
    formBranch.value = '';
    modalOpen.value = false;
  } catch (e) {
    creationError.value = e instanceof ApiError ? e.message : String(e);
  }
}

async function deleteProject(name: string) {
  await resource.remove(name);
}
</script>

<template>
  <div class="dashboard">
    <div class="dashboard__main">
      <header class="page-head">
        <h1>Projets</h1>
        <div class="page-actions">
          <button class="btn btn-create" @click="modalOpen = true">Nouveau projet</button>
          <button
            class="btn btn-assistant"
            :aria-expanded="assistantOpen"
            @click="assistantOpen = !assistantOpen"
          >
            Assistant
          </button>
        </div>
      </header>

      <LoadingSkeleton v-if="loading" />
      <div v-else>
        <ErrorCard v-if="error" :message="error" />
        <div v-else>
          <EmptyState v-if="fetchedProjects.length === 0" message="Aucun projet." />
          <ul v-else class="project-grid">
            <li v-for="p in fetchedProjects" :key="p.metadata.name" class="project-card">
              <div class="project-card__head">
                <button
                  type="button"
                  class="project-card__open"
                  @click="openProject(p.metadata.name)"
                >
                  {{ p.metadata.name }}
                </button>
                <details class="card-menu">
                  <summary class="card-menu__toggle" :aria-label="`Actions du projet ${p.metadata.name}`">…</summary>
                  <div class="card-menu__panel">
                    <button type="button" class="btn btn-delete" @click="deleteProject(p.metadata.name)">
                      Supprimer
                    </button>
                  </div>
                </details>
              </div>
              <p class="project-card__repo">{{ p.spec.repoUrl }}</p>
              <p class="project-card__state">
                <span class="dot" :class="`dot--${stateTone(p)}`" aria-hidden="true"></span>
                {{ cloneLabel(p) }}
              </p>
              <div v-if="isCloning(p)" class="clone-progress" role="progressbar" aria-label="Clonage en cours">
                <span class="clone-progress__bar"></span>
              </div>
              <div class="pills">
                <span
                  v-for="pill in sandboxPills(p.metadata.name)"
                  :key="pill.label"
                  class="pill"
                  :class="{ 'pill--active': pill.active }"
                >
                  {{ pill.label }}
                </span>
              </div>
            </li>
          </ul>
        </div>

        <DialogShell v-model:open="modalOpen" title="Créer un projet">
          <Field label="Nom">
            <input
              class="field-input"
              v-model="formName"
              type="text"
              placeholder="mon-projet"
              aria-label="Nom du projet"
            />
          </Field>
          <Field label="Repo URL">
            <input
              class="field-input"
              v-model="formRepo"
              type="text"
              placeholder="https://github.com/org/repo"
              aria-label="URL du dépôt"
            />
          </Field>
          <Field label="Branche par défaut">
            <input
              class="field-input"
              v-model="formBranch"
              type="text"
              placeholder="main (optionnel)"
              aria-label="Branche par défaut"
            />
          </Field>
          <div v-if="creationError" class="creation-error">{{ creationError }}</div>
          <template #actions>
            <button class="btn btn-create" @click="createProject">Créer</button>
            <DialogClose class="btn btn-cancel">Annuler</DialogClose>
          </template>
        </DialogShell>
      </div>
    </div>

    <AssistantPanel
      :open="assistantOpen"
      title="Assistant d'accueil"
      :data-layout="stacked ? 'sheet' : 'panel'"
      @close="assistantOpen = false"
    >
      <ContextChat kind="home" :data="homeChatData" />
    </AssistantPanel>
  </div>
</template>

<style scoped>
/* Coquille de l'écran : la page ne défile pas, la région de contenu défile en interne et
   l'assistant (AssistantPanel) occupe la colonne de droite à partir de md (layout.sdd). */
.dashboard {
  display: flex;
  height: 100%;
  min-height: 0;
  overflow: hidden;
  background: var(--bg);
  color: var(--text);
  font-family: var(--font-sans);
}

.dashboard__main {
  flex: 1;
  min-width: 0;
  min-height: 0;
  overflow: auto;
  padding: 24px 28px;
}

.page-head {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  margin-bottom: 20px;
}

h1 {
  font-size: 18px;
  font-weight: 600;
  margin: 0;
}

.page-actions {
  display: flex;
  gap: 8px;
}

.page-actions .btn {
  display: inline-flex;
  align-items: center;
  min-height: var(--ctl-h);
  padding: 0 12px;
  border-radius: 6px;
  border: 1px solid var(--border);
  font: inherit;
  font-weight: 500;
  cursor: pointer;
}

.page-actions .btn-create {
  background: var(--accent);
  border-color: var(--accent);
  color: var(--bg);
}

.page-actions .btn-create:hover {
  filter: brightness(1.08);
}

.page-actions .btn-assistant {
  background: var(--surface-2);
  color: var(--text);
}

.page-actions .btn-assistant:hover {
  background: var(--surface);
}

.page-actions .btn:focus-visible,
.project-card__open:focus-visible,
.card-menu__toggle:focus-visible {
  outline: 2px solid var(--accent);
  outline-offset: 2px;
}

/* Cartes : grille auto-fill, 260 px au minimum (maquette de la refonte). */
.project-grid {
  list-style: none;
  margin: 0;
  padding: 0;
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(260px, 1fr));
  gap: 16px;
}

.project-card {
  position: relative;
  display: flex;
  flex-direction: column;
  gap: 8px;
  min-width: 0;
  padding: 14px 16px;
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: 12px;
}

.project-card:hover {
  border-color: var(--accent);
}

.project-card__head {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 8px;
}

/* Le nom couvre toute la carte (lien étendu) ; le menu reste au-dessus. */
.project-card__open {
  appearance: none;
  padding: 0;
  border: none;
  background: none;
  color: var(--text);
  font: inherit;
  font-weight: 600;
  text-align: left;
  overflow-wrap: anywhere;
  cursor: pointer;
}

.project-card__open::after {
  content: '';
  position: absolute;
  inset: 0;
  border-radius: 12px;
}

.project-card__repo {
  margin: 0;
  font-family: var(--font-mono);
  font-size: 12px;
  color: var(--muted);
  overflow-wrap: anywhere;
}

.project-card__state {
  display: flex;
  align-items: center;
  gap: 6px;
  margin: 0;
  font-size: 12px;
  color: var(--muted);
}

.dot {
  display: inline-block;
  flex: none;
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: var(--muted);
}
.dot--ok {
  background: var(--ok);
}
.dot--warn {
  background: var(--warn);
}

/* Menu de la carte : pas de bouton rouge permanent, la suppression est repliée. */
.card-menu {
  position: relative;
  z-index: 1;
}

.card-menu__toggle {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: var(--ctl-h);
  height: var(--ctl-h);
  border-radius: 6px;
  color: var(--muted);
  cursor: pointer;
  list-style: none;
  user-select: none;
}

.card-menu__toggle::-webkit-details-marker {
  display: none;
}

.card-menu__toggle:hover {
  color: var(--text);
  background: var(--surface-2);
}

.card-menu__panel {
  position: absolute;
  right: 0;
  top: calc(var(--ctl-h) + 4px);
  z-index: 2;
  padding: 4px;
  background: var(--surface-2);
  border: 1px solid var(--border);
  border-radius: 8px;
}

.card-menu .btn-delete {
  display: block;
  min-height: var(--ctl-h);
  padding: 0 12px;
  border: none;
  border-radius: 6px;
  background: transparent;
  color: var(--err);
  font: inherit;
  white-space: nowrap;
  cursor: pointer;
}

.card-menu .btn-delete:hover {
  background: var(--surface);
}

/* Clonage en cours : barre fine, indéterminée (aucune mesure de progression disponible). */
.clone-progress {
  position: relative;
  height: 2px;
  overflow: hidden;
  border-radius: 2px;
  background: var(--surface-2);
}

.clone-progress__bar {
  position: absolute;
  inset: 0 auto 0 0;
  width: 35%;
  background: var(--accent);
  animation: clone-slide 1.4s ease-in-out infinite;
}

@keyframes clone-slide {
  from {
    transform: translateX(-100%);
  }
  to {
    transform: translateX(300%);
  }
}

@media (prefers-reduced-motion: reduce) {
  .clone-progress__bar {
    animation: none;
    width: 100%;
    opacity: 0.5;
  }
}

.pills {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  margin-top: auto;
}

.pill {
  display: inline-flex;
  align-items: center;
  padding: 2px 8px;
  border: 1px solid var(--border);
  border-radius: 999px;
  background: var(--surface-2);
  color: var(--muted);
  font-size: 11px;
  white-space: nowrap;
}

.pill--active {
  background: var(--accent-bg);
  border-color: transparent;
  color: var(--accent-text);
}

/* Sous md : marges de 16 px ; la feuille d'assistant est gérée par AssistantPanel. */
@media (max-width: 767px) {
  .dashboard__main {
    padding: 16px;
  }
}
</style>
