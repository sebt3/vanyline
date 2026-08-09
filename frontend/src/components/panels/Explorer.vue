<script setup lang="ts">
import { ElTree } from 'element-plus';

interface Node {
  id: string;
  label: string;
  leaf?: boolean;
  children?: Node[];
}

const data: Node[] = [
  {
    id: 'root',
    label: 'media-station',
    children: [
      {
        id: 'src',
        label: 'src',
        children: [
          { id: 'main.py', label: 'main.py', leaf: true },
          {
            id: 'jobs',
            label: 'jobs',
            children: [
              { id: 'sync_library.py', label: 'sync_library.py', leaf: true },
              { id: 'transcode.py', label: 'transcode.py', leaf: true },
            ],
          },
        ],
      },
      {
        id: 'workflows',
        label: 'workflows',
        children: [{ id: 'sync-media.dag.yaml', label: 'sync-media.dag.yaml', leaf: true }],
      },
      { id: 'README.md', label: 'README.md', leaf: true },
    ],
  },
];

const defaultExpandedKeys = ['root', 'src', 'jobs', 'workflows'];
</script>

<template>
  <div class="explorer">
    <el-tree
      :data="data"
      node-key="id"
      :default-expanded-keys="defaultExpandedKeys"
      current-node-key="sync_library.py"
      highlight-current
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
