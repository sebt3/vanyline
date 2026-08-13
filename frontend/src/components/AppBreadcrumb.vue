<script setup lang="ts">
import { computed } from 'vue';
import { useRoute } from 'vue-router';
import { activeNav } from './settings/navState';

const route = useRoute();

/** Segments : [libellé, chemin] — "Accueil" est toujours le premier segment. */
const segments = computed<[string, string][]>(() => {
  // '/'          → [['Accueil', '/']]
  // '/p/:project' → [['Accueil', '/'], [projectName, '/p/' + projectName]]
  // '/settings'   → [['Accueil', '/'], ['Paramètres', '/settings'],
  //                   [activeNav.groupLabel, '/settings'],
  //                   [activeNav.screenLabel, '/settings']]
  if (route.path === '/') {
    return [['Accueil', '/']];
  }

  if (route.name === 'project') {
    const projectName =
      typeof route.params.projectName === 'string' ? route.params.projectName : '';
    return [
      ['Accueil', '/'],
      [projectName, `/p/${projectName}`],
    ];
  }

  if (route.path === '/settings') {
    return [
      ['Accueil', '/'],
      ['Paramètres', '/settings'],
      [activeNav.value.groupLabel, '/settings'],
      [activeNav.value.screenLabel, '/settings'],
    ];
  }

  return [['Accueil', '/']];
});
</script>

<template>
  <nav class="breadcrumb">
    <template v-for="(segment, i) in segments" :key="segment[1]">
      <span v-if="i > 0" class="separator"> / </span>
      <router-link v-if="i < segments.length - 1" :to="segment[1]">
        {{ segment[0] }}
      </router-link>
      <span v-else class="current">{{ segment[0] }}</span>
    </template>
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

.breadcrumb .current {
  color: white;
}
</style>
