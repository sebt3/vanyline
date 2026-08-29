<script setup lang="ts">
interface Option {
  value: string;
  label: string;
}

const props = defineProps<{
  options: Option[];
  modelValue: string[];
}>();

const emit = defineEmits<{
  'update:modelValue': [value: string[]];
}>();

function toggle(value: string, checked: boolean) {
  const next = checked
    ? [...props.modelValue, value]
    : props.modelValue.filter((v) => v !== value);
  emit('update:modelValue', next);
}
</script>

<template>
  <div class="checkbox-list">
    <label v-for="opt in options" :key="opt.value" class="checkbox-item">
      <input
        type="checkbox"
        :value="opt.value"
        :checked="modelValue.includes(opt.value)"
        @change="toggle(opt.value, ($event.target as HTMLInputElement).checked)"
      />
      <span>{{ opt.label }}</span>
    </label>
  </div>
</template>

<style scoped>
.checkbox-list {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
}

.checkbox-item {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  font-size: 13px;
  color: #e6e9f0;
}

.checkbox-item input[type='checkbox'] {
  width: 14px;
  height: 14px;
}
</style>
