<script setup lang="ts">
import type { AdminUser } from '@serverrs/sdk'
import { Search, Trash2 } from '@lucide/vue'
import { computed, onMounted, ref } from 'vue'

import { api } from '@/lib/sdk'
import { errorMessage, formatDate, statusTone } from '@/utils/format'

const users = ref<AdminUser[]>([])
const loading = ref(false)
const savingId = ref<number | null>(null)
const error = ref('')
const search = ref('')
const page = ref(1)
const pageSize = 20
const total = ref(0)

const filteredUsers = computed(() => {
  const keyword = search.value.trim().toLowerCase()
  if (!keyword) return users.value
  return users.value.filter((user) =>
    [user.username, user.nickname, user.email, user.phone]
      .filter(Boolean)
      .some((value) => value!.toLowerCase().includes(keyword)),
  )
})

async function load() {
  loading.value = true
  error.value = ''
  try {
    const result = await api.admin.users({ page: page.value, pageSize })
    users.value = result.items
    total.value = result.total
  } catch (cause) {
    error.value = errorMessage(cause)
  } finally {
    loading.value = false
  }
}

async function updateStatus(user: AdminUser, value: string) {
  savingId.value = user.id
  try {
    const updated = await api.admin.updateUser(user.id, { status: value === 'active' ? 1 : 0 })
    Object.assign(user, updated)
  } catch (cause) {
    error.value = errorMessage(cause)
  } finally {
    savingId.value = null
  }
}

async function updateRole(user: AdminUser, value: string) {
  savingId.value = user.id
  try {
    const updated = await api.admin.updateUser(user.id, {
      role: value as 'USER' | 'ADMIN' | 'SUPER_ADMIN',
    })
    Object.assign(user, updated)
  } catch (cause) {
    error.value = errorMessage(cause)
  } finally {
    savingId.value = null
  }
}

async function removeUser(user: AdminUser) {
  if (!window.confirm(`确定删除用户“${user.username}”吗？此操作不可恢复。`)) return
  savingId.value = user.id
  try {
    await api.admin.deleteUser(user.id)
    await load()
  } catch (cause) {
    error.value = errorMessage(cause)
  } finally {
    savingId.value = null
  }
}

async function turnPage(next: number) {
  page.value = next
  await load()
}

onMounted(load)
</script>

<template>
  <div>
    <header class="page-heading">
      <div>
        <h1>用户管理</h1>
        <p>查看账号资料，控制启用状态并分配系统角色。</p>
      </div>
      <div class="toolbar">
        <div class="search-box">
          <Search :size="17" />
          <input v-model="search" class="input search-input" placeholder="筛选当前页用户" />
        </div>
        <button class="button" :disabled="loading" @click="load">刷新</button>
      </div>
    </header>

    <p v-if="error" class="notice">{{ error }}</p>

    <section class="card">
      <div v-if="loading" class="loading-state">正在加载用户...</div>
      <div v-else-if="!filteredUsers.length" class="empty-state">没有匹配的用户</div>
      <div v-else class="table-wrap">
        <table class="data-table">
          <thead>
            <tr><th>用户</th><th>联系方式</th><th>角色</th><th>状态</th><th>最近登录</th><th>操作</th></tr>
          </thead>
          <tbody>
            <tr v-for="user in filteredUsers" :key="user.id">
              <td>
                <div class="cell-title">
                  <strong>{{ user.nickname || user.username }}</strong>
                  <span>#{{ user.id }} · @{{ user.username }}</span>
                </div>
              </td>
              <td>
                <div class="cell-title">
                  <span>{{ user.email || '未设置邮箱' }}</span>
                  <span>{{ user.phone || '未设置手机' }}</span>
                </div>
              </td>
              <td>
                <select
                  class="select compact"
                  :disabled="savingId === user.id"
                  :value="user.role"
                  @change="updateRole(user, ($event.target as HTMLSelectElement).value)"
                >
                  <option value="USER">USER</option>
                  <option value="ADMIN">ADMIN</option>
                  <option value="SUPER_ADMIN">SUPER_ADMIN</option>
                </select>
              </td>
              <td>
                <select
                  class="select compact"
                  :class="statusTone(user.status)"
                  :disabled="savingId === user.id"
                  :value="user.status"
                  @change="updateStatus(user, ($event.target as HTMLSelectElement).value)"
                >
                  <option value="active">active</option>
                  <option value="disabled">disabled</option>
                </select>
              </td>
              <td>{{ formatDate(user.last_login_at) }}</td>
              <td>
                <button
                  class="button small danger"
                  :disabled="savingId === user.id"
                  @click="removeUser(user)"
                >
                  <Trash2 :size="14" />删除
                </button>
              </td>
            </tr>
          </tbody>
        </table>
      </div>
      <div class="pagination">
        <span>共 {{ total }} 位用户 · 第 {{ page }} 页</span>
        <button class="button small" :disabled="page <= 1 || loading" @click="turnPage(page - 1)">
          上一页
        </button>
        <button
          class="button small"
          :disabled="page * pageSize >= total || loading"
          @click="turnPage(page + 1)"
        >
          下一页
        </button>
      </div>
    </section>
  </div>
</template>

<style scoped>
.notice {
  margin-bottom: 16px;
}

.search-box {
  position: relative;
  display: flex;
  align-items: center;
}

.search-box svg {
  position: absolute;
  left: 12px;
  z-index: 1;
  color: var(--muted);
}

.search-box .input {
  padding-left: 36px;
}

.compact {
  min-width: 120px;
  min-height: 34px;
  font-size: 12px;
}

.compact.success {
  color: var(--success);
  background: var(--success-soft);
}

.compact.danger {
  color: var(--danger);
  background: var(--danger-soft);
}
</style>
