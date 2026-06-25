<script setup lang="ts">
import { X } from '@lucide/vue'

const props = withDefaults(defineProps<{
  open: boolean
  title: string
  message: string
  confirmLabel?: string
  cancelLabel?: string
  danger?: boolean
}>(), {
  confirmLabel: '确认',
  cancelLabel: '取消',
  danger: false,
})

const emit = defineEmits<{
  confirm: []
  cancel: []
}>()
</script>

<template>
  <Teleport to="body">
    <div v-if="open" class="modal-backdrop" @click.self="emit('cancel')">
      <div class="confirm-dialog">
        <div class="confirm-header">
          <h3>{{ title }}</h3>
          <button class="icon-button" type="button" @click="emit('cancel')">
            <X :size="18" />
          </button>
        </div>
        <p class="confirm-body">{{ message }}</p>
        <div class="confirm-actions">
          <button class="button" type="button" @click="emit('cancel')">{{ cancelLabel }}</button>
          <button class="button" :class="{ danger }" type="button" @click="emit('confirm')">
            {{ confirmLabel }}
          </button>
        </div>
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
.confirm-dialog {
  width: min(420px, 90vw);
  border-radius: var(--radius-lg);
  background: var(--surface);
  box-shadow: var(--shadow-lg);
}

.confirm-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 18px 20px;
  border-bottom: 1px solid var(--line);
}

.confirm-header h3 {
  font-size: 16px;
  font-weight: 750;
}

.confirm-body {
  padding: 20px;
  color: var(--muted);
  font-size: 14px;
  line-height: 1.6;
}

.confirm-actions {
  display: flex;
  justify-content: flex-end;
  gap: 10px;
  padding: 16px 20px;
  border-top: 1px solid var(--line);
}
</style>
