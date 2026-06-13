import type { AuthUser } from '@serverrs/sdk'
import { defineStore } from 'pinia'
import { computed, ref } from 'vue'

import { api, tokenStore } from '@/lib/sdk'

export const useAuthStore = defineStore('auth', () => {
  const user = ref<AuthUser | null>(null)
  const initialized = ref(false)
  const loading = ref(false)
  const isAdmin = computed(
    () => user.value?.role === 'ADMIN' || user.value?.role === 'SUPER_ADMIN',
  )

  async function restore() {
    if (initialized.value) return
    try {
      if (await tokenStore.getAccessToken()) {
        const current = await api.auth.me()
        if (current.role === 'ADMIN' || current.role === 'SUPER_ADMIN') user.value = current
        else await tokenStore.clear()
      }
    } catch {
      await tokenStore.clear()
    } finally {
      initialized.value = true
    }
  }

  async function login(username: string, password: string) {
    loading.value = true
    try {
      const result = await api.auth.login({ username, password })
      if (result.user.role !== 'ADMIN' && result.user.role !== 'SUPER_ADMIN') {
        await tokenStore.clear()
        throw new Error('该账号没有管理后台访问权限')
      }
      user.value = result.user
      initialized.value = true
    } finally {
      loading.value = false
    }
  }

  async function logout() {
    try {
      await api.auth.logout('admin_console_logout')
    } finally {
      user.value = null
      initialized.value = true
    }
  }

  return { user, initialized, loading, isAdmin, restore, login, logout }
})
