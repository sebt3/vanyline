<script setup lang="ts">
import { computed, inject, onMounted, ref } from 'vue';
import {
  gitClient,
  type GitStatus,
  type BranchesResult,
} from '../../api/gitClient';

const sandboxName = inject<string>('sandbox-name', '');

const status = ref<GitStatus | null>(null);
const branches = ref<BranchesResult | null>(null);
const commitMessage = ref('');
const busy = ref(false);
const errorMessage = ref<string | null>(null);

/** Fichiers staged (hors conflits — les conflicted sont staged par nature mais
 *  traités à part pour le geste « marquer résolu »). */
const stagedFiles = computed(() =>
  (status.value?.files ?? []).filter((f) => f.staged && f.state !== 'conflicted'),
);

/** Fichiers non staged (working tree) : modifiés/ajoutés/supprimés/renommés
 *  non staged + untracked. */
const unstagedFiles = computed(() =>
  (status.value?.files ?? []).filter((f) => !f.staged),
);

/** Fichiers en conflit (state === 'conflicted') — toujours staged. */
const conflictedFiles = computed(() =>
  (status.value?.files ?? []).filter((f) => f.state === 'conflicted'),
);

/** Un merge est en cours (MERGE_HEAD présent) — active le bouton
 *  « marquer résolu ». */
const merging = computed(() => branches.value?.merging ?? false);

/** Commit possible : message non vide ET quelque chose de staged (y compris
 *  les conflits). Désactivé sinon. */
const canCommit = computed(() =>
  commitMessage.value.trim() !== '' &&
  (stagedFiles.value.length + conflictedFiles.value.length) > 0,
);

/** Refetch status + branches (parallèle) — appelé au montage et après chaque
 *  action. Pas d'état local persistant au-delà des inputs de formulaire. */
async function refresh(): Promise<void> {
  if (!sandboxName) return;
  const [s, b] = await Promise.all([
    gitClient.status(sandboxName),
    gitClient.branches(sandboxName),
  ]);
  status.value = s;
  branches.value = b;
  errorMessage.value = null;
}

async function stageFile(path: string): Promise<void> {
  if (!sandboxName) return;
  busy.value = true;
  try {
    await gitClient.stage(sandboxName, [path]);
    await refresh();
  } catch (e) {
    errorMessage.value = msg(e);
  } finally {
    busy.value = false;
  }
}

async function unstageFile(path: string): Promise<void> {
  if (!sandboxName) return;
  busy.value = true;
  try {
    await gitClient.unstage(sandboxName, [path]);
    await refresh();
  } catch (e) {
    errorMessage.value = msg(e);
  } finally {
    busy.value = false;
  }
}

/** « Marquer résolu » = `/git/stage` sur un fichier conflicté (geste distinct
 *  du design, activé par `merging`). */
async function markResolved(path: string): Promise<void> {
  if (!sandboxName) return;
  busy.value = true;
  try {
    await gitClient.stage(sandboxName, [path]);
    await refresh();
  } catch (e) {
    errorMessage.value = msg(e);
  } finally {
    busy.value = false;
  }
}

async function commit(): Promise<void> {
  if (!sandboxName || !canCommit.value) return;
  busy.value = true;
  try {
    await gitClient.commit(sandboxName, commitMessage.value.trim());
    commitMessage.value = '';
    await refresh();
  } catch (e) {
    errorMessage.value = msg(e);
  } finally {
    busy.value = false;
  }
}

function msg(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

onMounted(() => { void refresh(); });

defineExpose({
  refresh, stageFile, unstageFile, markResolved, commit,
  stagedFiles, unstagedFiles, conflictedFiles, merging, canCommit,
});
</script>

<template>
  <div class="git-panel">
    <div v-if="!sandboxName" class="empty">Aucune sandbox…</div>
    <div v-if="errorMessage" class="git-error" role="alert">{{ errorMessage }}</div>
    <div v-if="busy && !status" class="empty">Chargement…</div>
    <template v-else-if="status">
      <section class="changes">
        <h3>Changements</h3>
        <p v-if="status.clean" class="clean">Aucun changement</p>
        <template v-else>
          <ul class="file-list">
            <li v-for="f in stagedFiles" :key="f.path" class="file staged">
              <span class="state">staged</span>
              <span class="path">{{ f.path }}</span>
              <button :disabled="busy" @click="unstageFile(f.path)">Retirer</button>
            </li>
            <li v-for="f in conflictedFiles" :key="f.path" class="file conflicted">
              <span class="state">conflit</span>
              <span class="path">{{ f.path }}</span>
              <button v-if="merging" :disabled="busy" @click="markResolved(f.path)">
                Marquer résolu
              </button>
            </li>
            <li v-for="f in unstagedFiles" :key="f.path" class="file unstaged">
              <span class="state">{{ f.state }}</span>
              <span class="path">{{ f.path }}</span>
              <button :disabled="busy" @click="stageFile(f.path)">Stager</button>
            </li>
          </ul>
        </template>
      </section>
      <section class="commit">
        <textarea
          v-model="commitMessage"
          rows="2"
          placeholder="Message de commit"
        />
        <button :disabled="busy || !canCommit" @click="commit">Commit</button>
      </section>
    </template>
  </div>
</template>

<style scoped>
.git-panel {
  height: 100%;
  overflow-y: auto;
  background: var(--dv-group-view-background-color);
  padding: 6px 0;
}
.git-error {
  padding: 6px 12px;
  margin: 0 8px 4px;
  background: #5b1e3fdd;
  color: #ffb4c8;
  font-size: 12px;
  border-radius: 6px;
}
.empty {
  padding: 8px 16px;
  color: var(--dv-color-abyss-secondary-text);
  font-size: 12px;
}
.changes {
  padding: 0 8px;
}
.changes h3 {
  font-size: 11px;
  font-weight: 600;
  text-transform: uppercase;
  color: var(--dv-color-abyss-secondary-text);
  margin: 8px 0 4px;
  letter-spacing: 0.5px;
}
.clean {
  padding: 8px 0;
  color: var(--dv-color-abyss-secondary-text);
  font-size: 12px;
  margin: 0;
}
.file-list {
  list-style: none;
  margin: 0;
  padding: 0;
}
.file {
  display: flex;
  align-items: center;
  gap: 4px;
  padding: 2px 8px;
  font-size: 12px;
  border-radius: 3px;
}
.file:hover {
  background: var(--dv-color-abyss-light);
}
.file .state {
  flex-shrink: 0;
  font-size: 10px;
  text-transform: uppercase;
  letter-spacing: 0.3px;
}
.file.staged .state {
  color: #7ecfff;
}
.file.conflicted .state {
  color: #ffb4c8;
}
.file.unstaged .state {
  color: var(--dv-color-abyss-secondary-text);
}
.file .path {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.file button {
  flex-shrink: 0;
  font-size: 11px;
  padding: 2px 8px;
  border-radius: 3px;
  border: 1px solid var(--dv-color-abyss-light);
  background: transparent;
  color: var(--dv-color-abyss-primary-text);
  cursor: pointer;
}
.file button:hover:not(:disabled) {
  background: var(--dv-color-abyss-lighter);
}
.file button:disabled {
  opacity: 0.4;
  cursor: not-allowed;
}
.file.conflicted {
  background: #5b1e3f22;
}
.commit {
  padding: 8px;
}
.commit textarea {
  width: 100%;
  box-sizing: border-box;
  background: var(--dv-color-abyss-light);
  border: 1px solid var(--dv-color-abyss-lighter);
  border-radius: 4px;
  color: var(--dv-color-abyss-primary-text);
  padding: 4px 8px;
  font-size: 12px;
  resize: vertical;
  font-family: inherit;
}
.commit button {
  margin-top: 4px;
  padding: 4px 12px;
  border-radius: 4px;
  border: 1px solid var(--dv-color-abyss-lighter);
  background: var(--dv-color-abyss-lighter);
  color: var(--dv-color-abyss-primary-text);
  font-size: 12px;
  cursor: pointer;
}
.commit button:hover:not(:disabled) {
  background: var(--dv-color-abyss-light);
}
.commit button:disabled {
  opacity: 0.4;
  cursor: not-allowed;
}
</style>