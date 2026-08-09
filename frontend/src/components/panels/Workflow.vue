<script setup lang="ts">
interface Node {
  id: string;
  label: string;
  status: 'done' | 'running' | 'pending';
  x: number;
  y: number;
}

const W = 150;
const H = 36;

const nodes: Node[] = [
  { id: 'fetch', label: 'fetch-metadata', status: 'done', x: 95, y: 10 },
  { id: 'transcode', label: 'transcode', status: 'done', x: 95, y: 78 },
  { id: 'thumbs', label: 'generate-thumbnails', status: 'running', x: 10, y: 146 },
  { id: 'subs', label: 'extract-subtitles', status: 'pending', x: 180, y: 146 },
  { id: 'publish', label: 'publish', status: 'pending', x: 95, y: 214 },
];

const cx = (n: Node) => n.x + W / 2;
const bottom = (n: Node) => n.y + H;

const statusLabel: Record<Node['status'], string> = {
  done: 'Terminé',
  running: 'En cours',
  pending: 'En attente',
};
</script>

<template>
  <div class="workflow">
    <div class="canvas">
      <svg width="340" height="260" class="edges">
        <path :d="`M ${cx(nodes[0])} ${bottom(nodes[0])} L ${cx(nodes[1])} ${nodes[1].y}`" />
        <path :d="`M ${cx(nodes[1])} ${bottom(nodes[1])} L ${cx(nodes[1])} 127 L ${cx(nodes[2])} 127 L ${cx(nodes[2])} ${nodes[2].y}`" />
        <path :d="`M ${cx(nodes[1])} ${bottom(nodes[1])} L ${cx(nodes[1])} 127 L ${cx(nodes[3])} 127 L ${cx(nodes[3])} ${nodes[3].y}`" />
        <path :d="`M ${cx(nodes[2])} ${bottom(nodes[2])} L ${cx(nodes[2])} 193 L ${cx(nodes[4])} 193 L ${cx(nodes[4])} ${nodes[4].y}`" />
        <path :d="`M ${cx(nodes[3])} ${bottom(nodes[3])} L ${cx(nodes[3])} 193 L ${cx(nodes[4])} 193 L ${cx(nodes[4])} ${nodes[4].y}`" />
      </svg>
      <div
        v-for="n in nodes"
        :key="n.id"
        class="node"
        :class="n.status"
        :style="{ left: `${n.x}px`, top: `${n.y}px`, width: `${W}px`, height: `${H}px` }"
      >
        <span class="dot" />
        <div class="node-text">
          <div class="node-label">{{ n.label }}</div>
          <div class="node-status">{{ statusLabel[n.status] }}</div>
        </div>
      </div>
    </div>
    <div class="meta">
      <span>DAG · sync-media</span>
      <span>déclenché il y a 4 min</span>
    </div>
  </div>
</template>

<style scoped>
.workflow {
  height: 100%;
  background: var(--dv-group-view-background-color);
  display: flex;
  flex-direction: column;
  align-items: center;
  padding: 20px;
  font-size: 12px;
  color: var(--dv-color-abyss-secondary-text);
}
.canvas {
  position: relative;
  width: 340px;
  height: 260px;
}
.edges path {
  fill: none;
  stroke: var(--dv-color-abyss-lighter);
  stroke-width: 1.5;
}
.node {
  position: absolute;
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 0 10px;
  border-radius: 3px;
  border: 1px solid var(--dv-color-abyss-lighter);
  background: var(--dv-color-abyss-light);
}
.node-label {
  color: var(--dv-color-abyss-primary-text);
  font-weight: 600;
  font-size: 11.5px;
}
.node-status {
  font-size: 10px;
  color: var(--dv-color-abyss-secondary-text);
}
.dot {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  flex: none;
}
.node.done .dot { background: #3fb56d; }
.node.done { border-color: #2c6b46; }
.node.running .dot {
  background: #e0a83d;
  animation: pulse 1.4s ease-in-out infinite;
}
.node.pending .dot { background: #5a6472; }
@keyframes pulse {
  0%, 100% { opacity: 1; }
  50% { opacity: 0.35; }
}
@media (prefers-reduced-motion: reduce) {
  .node.running .dot { animation: none; }
}
.meta {
  margin-top: 12px;
  display: flex;
  gap: 16px;
  font-family: ui-monospace, "SFMono-Regular", Menlo, Consolas, monospace;
  font-size: 11px;
}
</style>
