<script setup lang="ts">
import { computed, inject, onMounted, ref, watch } from 'vue';
import type { Ref } from 'vue';
import { createGitgraph } from '@gitgraph/js';
import {
  gitClient,
  type GitStatus,
  type BranchesResult,
  type LogResult,
  type UnpushedResult,
} from '../../api/gitClient';

const sandboxName = inject<string>('sandbox-name', '');

const openDiff = inject<(path: string, staged?: boolean) => void>('open-diff', () => {});

const fsVersion = inject<Ref<number> | null>('fs-version', null);
const notifyFsChange = inject<() => void>('notify-fs-change', () => {});

const status = ref<GitStatus | null>(null);
const branches = ref<BranchesResult | null>(null);
const commitMessage = ref('');
const busy = ref(false);
const errorMessage = ref<string | null>(null);
const unpushed = ref<UnpushedResult | null>(null);
const newBranchName = ref('');
const newBranchFrom = ref('');
const mergeBranch = ref('');

/** Container du graphe (template ref). */
const graphContainer = ref<HTMLElement | null>(null);
/** Dernier /git/log récupéré — source du graphe. */
const log = ref<LogResult | null>(null);

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

const currentBranch = computed(() => branches.value?.current ?? '');
const localBranches = computed(() =>
  (branches.value?.branches ?? []).filter((b) => !b.is_remote),
);
const remoteBranches = computed(() =>
  (branches.value?.branches ?? []).filter((b) => b.is_remote),
);
const unpushedCount = computed(() => unpushed.value?.commits.length ?? 0);

/** Refetch status + branches + unpushed + log (parallèle) — appelé au montage
 *  et après chaque action. Le graphe est re-rendu après chaque refetch. */
async function refresh(): Promise<void> {
  if (!sandboxName) return;
  const [s, b, u, l] = await Promise.all([
    gitClient.status(sandboxName),
    gitClient.branches(sandboxName),
    gitClient.unpushed(sandboxName),
    gitClient.log(sandboxName, 50, false),
  ]);
  status.value = s;
  branches.value = b;
  unpushed.value = u;
  log.value = l;
  errorMessage.value = null;
  renderGraph();
}

async function stageFile(path: string): Promise<void> {
  if (!sandboxName) return;
  busy.value = true;
  try {
    await gitClient.stage(sandboxName, [path]);
    await afterFsMutation();
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
    await afterFsMutation();
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
    await afterFsMutation();
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
    await afterFsMutation();
  } catch (e) {
    errorMessage.value = msg(e);
  } finally {
    busy.value = false;
  }
}

/** Crée une branche (from optionnel : branche locale ou remote). */
async function createBranch(): Promise<void> {
  if (!sandboxName || newBranchName.value.trim() === '') return;
  busy.value = true;
  try {
    const from = newBranchFrom.value.trim();
    await gitClient.createBranch(
      sandboxName,
      newBranchName.value.trim(),
      from || undefined,
    );
    newBranchName.value = '';
    newBranchFrom.value = '';
    await afterFsMutation();
  } catch (e) {
    errorMessage.value = msg(e);
  } finally {
    busy.value = false;
  }
}

async function checkout(branch: string): Promise<void> {
  if (!sandboxName) return;
  busy.value = true;
  try {
    await gitClient.checkout(sandboxName, branch);
    await afterFsMutation();
  } catch (e) {
    errorMessage.value = msg(e);
  } finally {
    busy.value = false;
  }
}

async function deleteBranch(branch: string): Promise<void> {
  if (!sandboxName) return;
  busy.value = true;
  try {
    await gitClient.deleteBranch(sandboxName, branch);
    await afterFsMutation();
  } catch (e) {
    errorMessage.value = msg(e);
  } finally {
    busy.value = false;
  }
}

async function push(): Promise<void> {
  if (!sandboxName) return;
  busy.value = true;
  try {
    await gitClient.push(sandboxName);
    await afterFsMutation();
  } catch (e) {
    errorMessage.value = msg(e);
  } finally {
    busy.value = false;
  }
}

/** Lance `git merge <branch>`. Un conflit est un résultat normal (réponse
 *  200 conflicted:true) : on refetch, le statut montre les fichiers en
 *  conflit et merging=true active « Marquer résolu ». Erreur HTTP seulement
 *  si le merge n'a pas pu être lancé (branche introuvable, merge déjà en
 *  cours, working tree sale) → errorMessage. */
async function mergeFrom(): Promise<void> {
  if (!sandboxName || mergeBranch.value.trim() === '' || merging.value) return;
  busy.value = true;
  try {
    await gitClient.merge(sandboxName, mergeBranch.value.trim());
    mergeBranch.value = '';
    await afterFsMutation();
  } catch (e) {
    errorMessage.value = msg(e);
  } finally {
    busy.value = false;
  }
}

/** Abandonne le merge en cours (`git merge --abort`). */
async function abortMerge(): Promise<void> {
  if (!sandboxName) return;
  busy.value = true;
  try {
    await gitClient.mergeAbort(sandboxName);
    await afterFsMutation();
  } catch (e) {
    errorMessage.value = msg(e);
  } finally {
    busy.value = false;
  }
}

function msg(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

/** Ouvre le diff d'un fichier. `staged=true` pour un fichier staged (hors
 *  conflit — HEAD → index) ; sans staged pour un fichier unstaged ou
 *  conflicté (index → working tree, marqueurs de conflit visibles). */
function openDiffFor(f: { path: string; state: string; staged: boolean }): void {
  openDiff(f.path, f.staged && f.state !== 'conflicted' ? true : undefined);
}

/** Rend le graphe linéaire de la branche courante (v1) : une branche, les
 *  commits dans l'ordre chronologique (le log est retourné du plus récent au
 *  plus ancien — inversion avant rendu), chaque commit décoré de ses refs. */
function renderGraph(): void {
  const container = graphContainer.value;
  const current = log.value;
  if (!container || !current) return;
  container.innerHTML = '';
  const gitgraph = createGitgraph(container);
  const branch = gitgraph.branch(current.branch || 'HEAD');
  const commits = [...current.commits].reverse();
  for (const c of commits) {
    const node = branch.commit({
      subject: c.title,
      hash: c.sha,
      author: c.author,
    });
    if (c.refs.length > 0) {
      node.tag(c.refs.join(', '));
    }
  }
}

if (fsVersion) {
  watch(fsVersion, () => { void refresh(); });
}

async function afterFsMutation(): Promise<void> {
  if (fsVersion) notifyFsChange();
  else await refresh();
}

onMounted(() => { void refresh(); });

defineExpose({
  refresh, stageFile, unstageFile, markResolved, commit,
  stagedFiles, unstagedFiles, conflictedFiles, merging, canCommit,
  createBranch, checkout, deleteBranch, push, mergeFrom, abortMerge,
  newBranchName, newBranchFrom, mergeBranch,
  currentBranch, localBranches, remoteBranches,
  unpushedCount, renderGraph, log, graphContainer,
  openDiffFor,
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
              <button :disabled="busy" @click="openDiffFor(f)">Diff</button>
              <button :disabled="busy" @click="unstageFile(f.path)">Retirer</button>
            </li>
            <li v-for="f in conflictedFiles" :key="f.path" class="file conflicted">
              <span class="state">conflit</span>
              <span class="path">{{ f.path }}</span>
              <button :disabled="busy" @click="openDiffFor(f)">Diff</button>
              <button v-if="merging" :disabled="busy" @click="markResolved(f.path)">
                Marquer résolu
              </button>
            </li>
            <li v-for="f in unstagedFiles" :key="f.path" class="file unstaged">
              <span class="state">{{ f.state }}</span>
              <span class="path">{{ f.path }}</span>
              <button :disabled="busy" @click="openDiffFor(f)">Diff</button>
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
      <section class="branches">
        <h3>Branches</h3>
        <div class="branch-current">
          Courante : <strong>{{ currentBranch }}</strong>
        </div>
        <div class="branch-create">
          <input v-model="newBranchName" placeholder="Nouvelle branche" />
          <input v-model="newBranchFrom" placeholder="Depuis (ex. origin/main)" />
          <button :disabled="busy || newBranchName.trim() === ''" @click="createBranch">
            Créer
          </button>
        </div>
        <ul class="branch-list">
          <li
            v-for="b in localBranches"
            :key="b.name"
            class="branch"
            :class="{ current: b.name === currentBranch }"
          >
            <span class="branch-name">{{ b.name }}</span>
            <span v-if="b.name === currentBranch" class="branch-mark">(courante)</span>
            <button :disabled="busy || b.name === currentBranch" @click="checkout(b.name)">
              Switcher
            </button>
            <button :disabled="busy || b.name === currentBranch" @click="deleteBranch(b.name)">
              Supprimer
            </button>
          </li>
          <li v-for="b in remoteBranches" :key="b.name" class="branch remote">
            <span class="branch-name">{{ b.name }}</span>
            <span class="branch-mark">(remote)</span>
          </li>
        </ul>
      </section>
      <section class="push">
        <h3>Push</h3>
        <p>
          {{ unpushedCount }} commit{{ unpushedCount === 1 ? '' : 's' }} non poussé{{ unpushedCount === 1 ? '' : 's' }}
          <span v-if="unpushed?.upstream">vers {{ unpushed.upstream }}</span>
        </p>
        <button :disabled="busy || unpushedCount === 0" @click="push">Push</button>
      </section>
      <section class="merge">
        <h3>Fusionner</h3>
        <div class="merge-form">
          <input v-model="mergeBranch" placeholder="Branche à fusionner" />
          <button :disabled="busy || merging || mergeBranch.trim() === ''" @click="mergeFrom">
            Fusionner
          </button>
        </div>
        <p v-if="merging" class="merge-in-progress">
          Merge en cours — résolvez les conflits puis committez, ou abandonnez.
        </p>
        <button v-if="merging" :disabled="busy" @click="abortMerge">Abandonner le merge</button>
      </section>
      <section class="graph">
        <h3>Historique</h3>
        <div ref="graphContainer" class="git-graph" />
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
.branches {
  padding: 8px;
}
.branches h3 {
  font-size: 11px;
  font-weight: 600;
  text-transform: uppercase;
  color: var(--dv-color-abyss-secondary-text);
  margin: 0 0 4px;
  letter-spacing: 0.5px;
}
.branch-current {
  font-size: 12px;
  padding: 2px 0 4px;
}
.branch-create {
  display: flex;
  gap: 4px;
  margin-bottom: 4px;
}
.branch-create input {
  flex: 1;
  min-width: 0;
  background: var(--dv-color-abyss-light);
  border: 1px solid var(--dv-color-abyss-lighter);
  border-radius: 4px;
  color: var(--dv-color-abyss-primary-text);
  padding: 2px 6px;
  font-size: 11px;
}
.branch-create button {
  padding: 2px 8px;
  font-size: 11px;
  border-radius: 3px;
  border: 1px solid var(--dv-color-abyss-light);
  background: transparent;
  color: var(--dv-color-abyss-primary-text);
  cursor: pointer;
}
.branch-create button:hover:not(:disabled) {
  background: var(--dv-color-abyss-lighter);
}
.branch-create button:disabled {
  opacity: 0.4;
  cursor: not-allowed;
}
.branch-list {
  list-style: none;
  margin: 0;
  padding: 0;
}
.branch {
  display: flex;
  align-items: center;
  gap: 4px;
  padding: 2px 8px;
  font-size: 12px;
  border-radius: 3px;
}
.branch:hover {
  background: var(--dv-color-abyss-light);
}
.branch.current {
  background: #7ecfff22;
}
.branch .branch-name {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.branch .branch-mark {
  flex-shrink: 0;
  font-size: 10px;
  color: var(--dv-color-abyss-secondary-text);
}
.branch .branch button {
  flex-shrink: 0;
  font-size: 10px;
  padding: 1px 6px;
  border-radius: 3px;
  border: 1px solid var(--dv-color-abyss-light);
  background: transparent;
  color: var(--dv-color-abyss-primary-text);
  cursor: pointer;
}
.branch .branch button:hover:not(:disabled) {
  background: var(--dv-color-abyss-lighter);
}
.branch .branch button:disabled {
  opacity: 0.4;
  cursor: not-allowed;
}
.push {
  padding: 8px;
}
.push h3 {
  font-size: 11px;
  font-weight: 600;
  text-transform: uppercase;
  color: var(--dv-color-abyss-secondary-text);
  margin: 0 0 4px;
  letter-spacing: 0.5px;
}
.push p {
  font-size: 12px;
  margin: 0 0 4px;
}
.push button {
  padding: 4px 12px;
  border-radius: 4px;
  border: 1px solid var(--dv-color-abyss-lighter);
  background: var(--dv-color-abyss-lighter);
  color: var(--dv-color-abyss-primary-text);
  font-size: 12px;
  cursor: pointer;
}
.push button:hover:not(:disabled) {
  background: var(--dv-color-abyss-light);
}
.push button:disabled {
  opacity: 0.4;
  cursor: not-allowed;
}
.merge { padding: 8px; }
.merge h3 { font-size: 11px; font-weight: 600; text-transform: uppercase; color: var(--dv-color-abyss-secondary-text); margin: 0 0 4px; letter-spacing: 0.5px; }
.merge-form { display: flex; gap: 4px; }
.merge-form input { flex: 1; min-width: 0; background: var(--dv-color-abyss-light); border: 1px solid var(--dv-color-abyss-lighter); border-radius: 4px; color: var(--dv-color-abyss-primary-text); padding: 2px 6px; font-size: 11px; }
.merge-form button { padding: 2px 8px; font-size: 11px; border-radius: 3px; border: 1px solid var(--dv-color-abyss-light); background: transparent; color: var(--dv-color-abyss-primary-text); cursor: pointer; }
.merge-form button:hover:not(:disabled) { background: var(--dv-color-abyss-lighter); }
.merge-form button:disabled { opacity: 0.4; cursor: not-allowed; }
.merge-in-progress { font-size: 12px; margin: 0 0 4px; }
.graph { padding: 8px; }
.graph h3 { font-size: 11px; font-weight: 600; text-transform: uppercase; color: var(--dv-color-abyss-secondary-text); margin: 0 0 4px; letter-spacing: 0.5px; }
.git-graph { overflow-x: auto; }
.git-graph svg { display: block; max-width: 100%; }
</style>