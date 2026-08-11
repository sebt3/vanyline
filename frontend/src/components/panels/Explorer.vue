<script setup lang="ts">
import { inject, computed, ref, watch } from 'vue';
import { ElTree } from 'element-plus';
import type { SandboxFsClient } from '../../api/sandboxWs';
import type { Ref } from 'vue';

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
 *  chemin relatif du dossier. */
async function loadNode(
  node: { data: unknown },
  resolve: (data: FsNode[]) => void,
  reject: (e: Error) => void,
): Promise<void> {
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

// Exposition des fonctions internes pour les tests unitaires
defineExpose({ parseEntries, loadNode, onNodeClick });
</script>

<template>
  <div class="explorer">
    <div v-if="!fsClient" class="empty">Connexion à la sandbox…</div>
    <el-tree
      v-else
      :key="'fs-' + (fsClient ? 'ready' : 'pending')"
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
        <span class="label">{{ node.label }}</span>
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
.label {
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
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