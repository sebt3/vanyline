<script setup lang="ts">
import { computed } from 'vue';
import { useRoute } from 'vue-router';

// Contrat : ./shell.sdd. Présentationnel : aucune logique métier, aucune API.
const route = useRoute();

const projetsActive = computed(() => route.path === '/' || route.path.startsWith('/p/'));
const reglagesActive = computed(() => route.path.startsWith('/settings'));
</script>

<template>
  <nav class="app-nav" aria-label="Navigation principale">
    <ul class="app-nav-list">
      <li>
        <router-link v-slot="{ href, navigate }" to="/" custom>
          <a
            class="app-nav-link"
            :href="href"
            :aria-current="projetsActive ? 'page' : undefined"
            @click="navigate"
          >
            <svg class="app-nav-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" focusable="false">
              <path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" />
            </svg>
            <span class="app-nav-label">Projets</span>
          </a>
        </router-link>
      </li>
    </ul>
    <ul class="app-nav-list app-nav-list--bottom">
      <li>
        <router-link v-slot="{ href, navigate }" to="/settings" custom>
          <a
            class="app-nav-link"
            :href="href"
            :aria-current="reglagesActive ? 'page' : undefined"
            @click="navigate"
          >
            <svg class="app-nav-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" focusable="false">
              <circle cx="12" cy="12" r="3" />
              <path d="M19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.1a1.7 1.7 0 0 0-1.1-1.5 1.7 1.7 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.8 1.7 1.7 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.1a1.7 1.7 0 0 0 1.5-1.1 1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 1.8.3H9a1.7 1.7 0 0 0 1-1.5V3a2 2 0 1 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.8V9a1.7 1.7 0 0 0 1.5 1H21a2 2 0 1 1 0 4h-.1a1.7 1.7 0 0 0-1.5 1z" />
            </svg>
            <span class="app-nav-label">Paramètres</span>
          </a>
        </router-link>
      </li>
    </ul>
  </nav>
</template>

<style scoped>
/* Rail vertical 60 px à partir de md (768 px) ; barre d'onglets en bas en dessous. */
.app-nav {
  flex: none;
  width: 60px;
  height: 100%;
  display: flex;
  flex-direction: column;
  justify-content: space-between;
  background: var(--surface);
  border-right: 1px solid var(--border);
  position: relative;
  font-family: var(--font-sans);
}
.app-nav-list {
  list-style: none;
  margin: 0;
  padding: 6px 0;
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.app-nav-list--bottom {
  padding-bottom: 8px;
}
.app-nav-link {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 2px;
  min-height: var(--ctl-h);
  margin: 0 6px;
  padding: 6px 0;
  border-radius: 6px;
  color: var(--muted);
  text-decoration: none;
}
.app-nav-link:hover {
  color: var(--text);
  background: var(--surface-2);
}
.app-nav-link[aria-current='page'] {
  color: var(--accent-text);
  background: var(--accent-bg);
}
.app-nav-link:focus-visible {
  outline: 2px solid var(--accent);
  outline-offset: -2px;
}
.app-nav-icon {
  width: 20px;
  height: 20px;
  flex: none;
}
/* Sur le rail, le libellé reste accessible (nom du lien) sans être affiché. */
.app-nav-label {
  position: absolute;
  width: 1px;
  height: 1px;
  overflow: hidden;
  clip: rect(0 0 0 0);
  white-space: nowrap;
}

@media (max-width: 767px) {
  .app-nav {
    width: 100%;
    height: var(--nav-h);
    flex-direction: row;
    justify-content: stretch;
    border-right: none;
    border-top: 1px solid var(--border);
  }
  .app-nav-list {
    flex: 1;
    flex-direction: row;
    padding: 0;
    gap: 0;
  }
  .app-nav-list--bottom {
    padding-bottom: 0;
  }
  .app-nav-list > li {
    flex: 1;
    display: flex;
  }
  .app-nav-link {
    flex: 1;
    min-height: var(--ctl-h);
    margin: 0;
    padding: 4px 0;
    font-size: 11px;
    border-radius: 0;
  }
  .app-nav-label {
    position: static;
    width: auto;
    height: auto;
    clip: auto;
    overflow: visible;
  }
}
</style>
