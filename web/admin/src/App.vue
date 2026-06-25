<script setup lang="ts">
import ToastProvider from './components/ToastProvider.vue'
</script>

<template>
  <RouterView />
  <ToastProvider />
</template>
