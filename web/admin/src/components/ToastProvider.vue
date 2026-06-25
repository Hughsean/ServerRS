<script setup lang="ts">
import { TransitionGroup } from 'vue'
import { useToast } from '../utils/toast'

const { toasts } = useToast()
</script>

<template>
  <Teleport to="body">
    <div class="toast-container">
      <TransitionGroup name="toast">
        <div v-for="t in toasts" :key="t.id" class="toast" :class="t.type">
          <span>{{ t.message }}</span>
        </div>
      </TransitionGroup>
    </div>
  </Teleport>
</template>

<style scoped>
.toast-container {
  position: fixed;
  top: 20px;
  right: 20px;
  z-index: 999;
  display: flex;
  flex-direction: column;
  gap: 10px;
  pointer-events: none;
}

.toast {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 12px 18px;
  border-radius: var(--radius-md);
  background: var(--surface);
  box-shadow: var(--shadow-lg);
  font-size: 13px;
  font-weight: 600;
  pointer-events: auto;
  max-width: 400px;
}

.toast.success { border-left: 4px solid var(--success); color: var(--success); }
.toast.error { border-left: 4px solid var(--danger); color: var(--danger); }
.toast.info { border-left: 4px solid var(--brand); color: var(--brand); }
.toast.warning { border-left: 4px solid var(--warning); color: var(--warning); }

.toast-enter-active,
.toast-leave-active {
  transition: all 250ms ease;
}
.toast-enter-from { opacity: 0; transform: translateX(40px); }
.toast-leave-to { opacity: 0; transform: translateX(40px); }
</style>
