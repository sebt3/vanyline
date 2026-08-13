<script setup lang="ts">
import { computed } from 'vue';
import { useRoute } from 'vue-router';

const route = useRoute();

/** Segments : [libellé, chemin] — "Accueil" est toujours le premier segment. */
const segments = computed<[string, string][]>(() => {
  // '/'          → [['Accueil', '/']]
  // '/p/:project' → [['Accueil', '/'], [projectName, '/p/' + projectName]]
  // '/settings'   → [['Accueil', '/'], ['Paramètres', '/settings']]
  if (route.path === '/') {
    return [['Accueil', '/']];
  }

  if (route.name === 'project') {
    const projectName = typeof route.params.projectName === 'string' ? route.params.projectName : '';
    return [
      ['Accueil', '/'],
      [projectName, `/p/${projectName}`],
    ];
  }

  if (route.name === 'ide') {
    return [
      ['Accueil', '/'],
      [
        typeof route.params.projectName === 'string' ? route.params.projectName : '',
        `/p/${typeof route.params.projectName === 'string' ? route.params.projectName : ''}`,
      ],
      [
        typeof route.params.sandboxName === 'string' ? route.params.sandboxName : '',
        `/p/${typeof route.params.projectName === 'string' ? route.params.projectName : ''}/s/${typeof route.params.sandboxName === 'string' ? route.params.sandboxName : ''}`,
      ],
    ];
  }

  if (route.path === '/settings') {
    return [
      ['Accueil', '/'],
      ['Paramètres', '/settings'],
    ];
  }

  return [['Accueil', '/']];
});
</script>

<template>
  <nav class="breadcrumb">
    <router-link
      v-for="segment in segments"
      :key="segment[1]"
      :to="segment[1]"
    >
      {{ segment[0] }}
    </router-link>
    <span v-if="segments.length > 1" class="separator"> / </span>
  </nav>
</template>

<style scoped>
.breadcrumb {
  display: flex;
  align-items: center;
  gap: 4px;
  font-size: 13px;
}

.breadcrumb a {
  color: rgba(255, 255, 255, 0.75);
  text-decoration: none;
}

.breadcrumb a:hover {
  color: white;
}

.breadcrumb .separator {
  color: rgba(255, 255, 255, 0.35);
  padding: 0 4px;
}
</style>