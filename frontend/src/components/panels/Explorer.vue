<script setup lang="ts">
import { inject, computed, ref, watch } from 'vue';
import { ElTree } from 'element-plus';
import type { SandboxFsClient } from '../../api/sandboxWs';
import type { Ref } from 'vue';
import ContextMenu, { type ContextMenuEntry } from '../ContextMenu.vue';
import { iconForPath, folderIcon } from './fileIcon';

interface FsNode {
  id: string;        // chemin relatif (unique dans l'arbre)
  label: string;    // nom de l'entrée
  path: string;     // chemin relatif pour les requêtes /ws/fs
  leaf?: boolean;   // true = fichier, false/absent = dossier
  children?: FsNode[];
}

// Fournis par IdeShell (task-05) :
// - 'sandbox-fs' : Ref<SandboxFsClient | null>
// - 'sandbox-name' : string
// - 'open-file' : (path: string) => void
const fsClient = inject<Ref<SandboxFsClient | null>>('sandbox-fs', ref(null) as Ref<SandboxFsClient | null>);
const sandboxName = inject<string>('sandbox-name', '');
const openFile = inject<(path: string) => void>('open-file', () => {});

// Fourni par IdeShell : ferme l'onglet Editor d'un chemin s'il est ouvert.
const closeFile = inject<(path: string) => void>('close-file', () => {});

const refreshKey = ref(0);
const errorMessage = ref<string | null>(null);

const rootNode = computed(() => ({
  id: '.',
  label: sandboxName,
  path: '.',
  leaf: false,
  children: [],
}));

// treeData : tableau brut (sans ref) que element-plus peut lire directement.
// watch immediate copie la valeur de rootNode au montage.
const treeData: FsNode[] = [];
watch(
  rootNode,
  (val) => {
    treeData[0] = val;
  },
  { immediate: true },
);

/** Join un chemin relatif parent + nom d'entrée. La racine est '.', les chemins
 *  produits sont relatifs à sandbox_root (résolus par confine_path côté serveur). */
function joinPath(parent: string, name: string): string {
  const base = parent === '.' ? '' : parent;
  return base ? `${base}/${name}` : name;
}

/** Parent relatif d'un chemin (« a/b/c » → « a/b »), la racine « . » retourne « . ». */
function parentPath(path: string): string {
  const idx = path.lastIndexOf('/');
  return idx === -1 ? '.' : path.slice(0, idx);
}

/** Parse le texte `entries` de l'op `list` (list_directory, depth 1 → plat) :
 *  une ligne = une entrée ; dossiers finissent par '/', fichiers non. On ignore
 *  les lignes marqueurs ("X is empty" pour un dossier vide, "[truncated…"). */
function parseEntries(parentPath: string, text: string): FsNode[] {
  const nodes: FsNode[] = [];
  for (const line of text.split('\n')) {
    const trimmed = line.trim();
    if (trimmed === '') continue;
    if (trimmed.endsWith(' is empty') || trimmed.startsWith('[truncated')) continue;
    const isDir = trimmed.endsWith('/');
    const name = isDir ? trimmed.slice(0, -1) : trimmed;
    nodes.push({
      id: joinPath(parentPath, name),
      label: name,
      path: joinPath(parentPath, name),
      leaf: !isDir,
      children: isDir ? [] : undefined,
    });
  }
  return nodes;
}

/** load el-tree : une requête list par dossier déplié. node.data.path est le
 *  chemin relatif du dossier — SAUF le tout premier appel : un arbre `lazy`
 *  fait charger sa propre racine invisible avant celle de nos données
 *  (`TreeStore.initialize()`, element-plus), et pour cet appel-là
 *  `node.data` est le tableau `treeData` passé en prop (pas un FsNode) — on
 *  le lui rend tel quel (c'est déjà exactement l'entrée racine "."), sans
 *  round-trip serveur. Sans ce cas, `data.path` vaut `undefined` : le
 *  serveur répond `{"error":"missing path","ok":false}` et l'arbre affiche
 *  "No Data" sans autre signal. */
async function loadNode(
  node: { data: unknown },
  resolve: (data: FsNode[]) => void,
  reject: (e: Error) => void,
): Promise<void> {
  if (Array.isArray(node.data)) {
    resolve(node.data as FsNode[]);
    return;
  }
  const data = node.data as FsNode;
  const fs = fsClient.value;
  if (!fs) {
    reject(new Error('fs client not ready'));
    return;
  }
  try {
    const resp = await fs.request<{ ok: boolean; entries: string }>('list', {
      path: data.path,
    });
    resolve(parseEntries(data.path, resp.entries));
  } catch (e) {
    reject(e instanceof Error ? e : new Error(String(e)));
  }
}

function onNodeClick(data: FsNode) {
  if (data.leaf) openFile(data.path);
}

/** Entrées du menu contextuel selon le type de nœud :
 *  - racine ('.') : Nouveau fichier, Nouveau dossier ;
 *  - dossier (non racine) : Nouveau fichier, Nouveau dossier, Copier les chemins,
 *    Renommer, Supprimer ;
 *  - fichier : Copier les chemins, Renommer, Supprimer. */
function entriesForNode(node: FsNode): ContextMenuEntry[] {
  const entries: ContextMenuEntry[] = [];
  if (!node.leaf) {
    entries.push({ label: 'Nouveau fichier', action: () => createFile(node.path) });
    entries.push({ label: 'Nouveau dossier', action: () => createDir(node.path) });
  }
  if (node.path !== '.') {
    entries.push({ sep: true });
    entries.push({ label: 'Copier le chemin relatif', action: () => copyRelativePath(node) });
    entries.push({ label: 'Copier le chemin absolu', action: () => copyAbsolutePath(node) });
    entries.push({ sep: true });
    entries.push({ label: 'Renommer', action: () => renameNode(node) });
    entries.push({ label: 'Supprimer', action: () => deleteNode(node) });
  }
  return entries;
}

function refresh(): void { refreshKey.value += 1; }
function setError(message: string): void { errorMessage.value = message; }

function createFile(dirPath: string): void {
  const name = window.prompt('Nom du fichier');
  if (!name) return;
  const fs = fsClient.value;
  if (!fs) return;
  fs.request<{ ok: boolean }>('write', { path: joinPath(dirPath, name), content: '' })
    .then(() => { errorMessage.value = null; refresh(); })
    .catch((e: unknown) => setError(`Création impossible : ${msg(e)}`));
}

function createDir(dirPath: string): void {
  const name = window.prompt('Nom du dossier');
  if (!name) return;
  const fs = fsClient.value;
  if (!fs) return;
  fs.request<{ ok: boolean }>('mkdir', { path: joinPath(dirPath, name) })
    .then(() => { errorMessage.value = null; refresh(); })
    .catch((e: unknown) => setError(`Création impossible : ${msg(e)}`));
}

function renameNode(node: FsNode): void {
  const name = window.prompt('Nouveau nom', node.label);
  if (!name || name === node.label) return;
  const fs = fsClient.value;
  if (!fs) return;
  const to = joinPath(parentPath(node.path), name);
  fs.request<{ ok: boolean }>('rename', { path: node.path, to })
    .then(() => {
      errorMessage.value = null;
      if (node.leaf) closeFile(node.path);
      refresh();
    })
    .catch((e: unknown) => setError(`Renommage impossible : ${msg(e)}`));
}

function deleteNode(node: FsNode): void {
  // Irréversible côté sandbox (pas de corbeille) : confirmation systématique
  // avant d'envoyer l'op.
  if (!window.confirm(`Supprimer « ${node.label} » ? Cette action est irréversible.`)) return;
  const fs = fsClient.value;
  if (!fs) return;
  fs.request<{ ok: boolean; error?: string }>('delete', { path: node.path })
    .then((resp) => {
      if (resp.ok) {
        errorMessage.value = null;
        if (node.leaf) closeFile(node.path);
        refresh();
      } else {
        setError(`Suppression impossible : ${resp.error ?? 'inconnue'}`);
      }
    })
    .catch((e: unknown) => setError(`Suppression impossible : ${msg(e)}`));
}

function msg(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

// Racine absolue du sandbox (op `root`) — invariante pour la durée de la
// session, récupérée une seule fois et mise en cache.
let sandboxRootPromise: Promise<string | null> | undefined;
function fetchSandboxRoot(): Promise<string | null> {
  const fs = fsClient.value;
  if (!fs) return Promise.resolve(null);
  if (!sandboxRootPromise) {
    sandboxRootPromise = fs
      .request<{ ok: boolean; root: string }>('root', {})
      .then((resp) => resp.root)
      .catch((e: unknown) => {
        setError(`Chemin absolu indisponible : ${msg(e)}`);
        sandboxRootPromise = undefined;
        return null;
      });
  }
  return sandboxRootPromise;
}

function writeClipboard(text: string): void {
  if (!navigator.clipboard) { setError('Presse-papiers indisponible'); return; }
  navigator.clipboard.writeText(text)
    .then(() => { errorMessage.value = null; })
    .catch((e: unknown) => setError(`Copie impossible : ${msg(e)}`));
}

function copyRelativePath(node: FsNode): void {
  writeClipboard(node.path);
}

function copyAbsolutePath(node: FsNode): void {
  fetchSandboxRoot().then((root) => {
    if (root) writeClipboard(`${root}/${node.path}`);
  });
}

// Exposition des fonctions internes pour les tests unitaires
defineExpose({
  parseEntries, loadNode, onNodeClick, entriesForNode, createFile, createDir,
  renameNode, deleteNode, refresh, parentPath, copyRelativePath, copyAbsolutePath,
});
</script>

<template>
  <div class="explorer">
    <div v-if="!fsClient" class="empty">Connexion à la sandbox…</div>
    <div v-if="errorMessage" class="explorer-error" role="alert">{{ errorMessage }}</div>
    <el-tree
      v-else
      :key="refreshKey + '-' + (fsClient ? 'ready' : 'pending')"
      :data="treeData"
      node-key="id"
      lazy
      :load="loadNode"
      :props="{ label: 'label', isLeaf: 'leaf', children: 'children' }"
      :default-expanded-keys="['.']"
      highlight-current
      @node-click="onNodeClick"
    >
      <template #default="{ data: node }">
        <ContextMenu :entries="entriesForNode(node)">
          <span class="label">
            <component :is="node.leaf ? iconForPath(node.path) : folderIcon" class="file-icon" />
            {{ node.label }}
          </span>
        </ContextMenu>
      </template>
    </el-tree>
  </div>
</template>

<style scoped>
.explorer {
  height: 100%;
  overflow-y: auto;
  background: var(--dv-group-view-background-color);
  padding: 6px 0;
}
.explorer-error {
  padding: 6px 12px;
  margin: 0 8px 4px;
  background: #5b1e3fdd;
  color: #ffb4c8;
  font-size: 12px;
  border-radius: 6px;
}
.label {
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.file-icon {
  width: 13px;
  height: 13px;
  margin-right: 4px;
  vertical-align: -2px;
  color: var(--dv-color-abyss-secondary-text);
}

.explorer :deep(.el-tree) {
  --el-tree-node-content-height: 22px;
  --el-tree-node-hover-bg-color: var(--dv-color-abyss-light);
  --el-tree-text-color: var(--dv-color-abyss-secondary-text);
  --el-tree-expand-icon-color: var(--dv-color-abyss-secondary-text);
  background: transparent;
  font-size: 12.5px;
}
.explorer :deep(.el-tree-node__content) {
  padding-right: 10px;
  border-radius: 3px;
}
.explorer :deep(.el-tree-node__expand-icon svg) {
  width: 9px;
  height: 9px;
}
.explorer :deep(.el-tree--highlight-current .el-tree-node.is-current > .el-tree-node__content) {
  background: var(--dv-color-abyss-lighter);
}
.explorer :deep(.el-tree--highlight-current .el-tree-node.is-current > .el-tree-node__content .label) {
  color: var(--dv-color-abyss-primary-text);
}
</style>
