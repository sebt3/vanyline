<script setup lang="ts">
import { onMounted, ref } from 'vue';
import { ApiError, createApiClient } from '../../api/client';

interface Me {
  email: string;
  k8s_owner_name: string | null;
}

const client = createApiClient();
const me = ref<Me | null>(null);
const error = ref<string | null>(null);
const loading = ref(true);

onMounted(async () => {
  try {
    me.value = await client.get<Me>('/api/me');
  } catch (e) {
    error.value = e instanceof ApiError ? e.message : String(e);
  } finally {
    loading.value = false;
  }
});
</script>

<template>
  <div class="card" v-if="loading">
    <div class="skeleton" />
    <div class="skeleton short" />
    <div class="skeleton short" />
  </div>
  <div v-else>
    <div class="card" v-if="error" role="alert">
      <p class="error-text">{{ error }}</p>
    </div>
    <template v-else-if="me">
      <div class="card">
        <label class="field">
          <span class="field-label">E-mail</span>
          <span class="field-value">{{ me.email }}</span>
        </label>
        <label class="field">
          <span class="field-label">Owner K8s</span>
          <span class="field-value">
            {{ me.k8s_owner_name ?? 'pas encore provisionné' }}
          </span>
        </label>
      </div>
    </template>
  </div>
</template>

<style scoped>
.card {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 22px 28px;
  max-width: 760px;
  padding: 28px 32px;
  background: #101828;
  border: 1px solid #1c1c2a;
  border-radius: 10px;
}

.skeleton {
  height: 16px;
  border-radius: 4px;
  background: linear-gradient(90deg, #1a2332 25%, #1f2b3d 50%, #1a2332 75%);
  background-size: 200% 100%;
  animation: shimmer 1.5s infinite;
}
.skeleton.short {
  width: 60%;
}

.error-text {
  color: #e85d5d;
  font-size: 13px;
  margin: 0;
}
</style>