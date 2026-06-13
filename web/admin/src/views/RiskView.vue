<script setup lang="ts">
import type { Conversation } from '@serverrs/sdk'
import { ArrowUpRight, MessagesSquare } from '@lucide/vue'
import { onMounted, ref } from 'vue'

import { api } from '@/lib/sdk'
import { errorMessage, formatDate } from '@/utils/format'

const items = ref<Conversation[]>([])
const loading = ref(false)
const error = ref('')
const riskLevel = ref('')
const page = ref(1)
const pageSize = 20
const total = ref(0)

async function load(reset = false) {
  if (reset) page.value = 1
  loading.value = true
  error.value = ''
  try {
    const result = await api.admin.riskConversations({
      page: page.value,
      pageSize,
      riskLevel: riskLevel.value || undefined,
    })
    items.value = result.items
    total.value = result.total
  } catch (cause) {
    error.value = errorMessage(cause)
  } finally {
    loading.value = false
  }
}

async function turnPage(next: number) {
  page.value = next
  await load()
}

onMounted(() => load())
</script>

<template>
  <div>
    <header class="page-heading">
      <div>
        <h1>风险会话</h1>
        <p>定位被风险检测命中的对话，查看消息上下文并记录处置结果。</p>
      </div>
      <div class="toolbar">
        <select v-model="riskLevel" class="select risk-select" @change="load(true)">
          <option value="">全部风险等级</option>
          <option value="none">none</option>
          <option value="low">low</option>
          <option value="medium">medium</option>
          <option value="high">high</option>
          <option value="critical">critical</option>
        </select>
        <button class="button" :disabled="loading" @click="load()">刷新</button>
      </div>
    </header>

    <p v-if="error" class="notice">{{ error }}</p>

    <section class="card">
      <div v-if="loading" class="loading-state">正在加载风险会话...</div>
      <div v-else-if="!items.length" class="empty-state">
        <div><MessagesSquare :size="34" /><p>当前筛选条件下没有风险会话</p></div>
      </div>
      <div v-else class="table-wrap">
        <table class="data-table">
          <thead>
            <tr><th>会话</th><th>用户 ID</th><th>消息数</th><th>最后消息</th><th>创建时间</th><th></th></tr>
          </thead>
          <tbody>
            <tr v-for="item in items" :key="item.id">
              <td>
                <div class="cell-title">
                  <strong>{{ item.title || `会话 #${item.id}` }}</strong>
                  <span>Conversation ID: {{ item.id }}</span>
                </div>
              </td>
              <td>#{{ item.user_id }}</td>
              <td>{{ item.message_count }}</td>
              <td>{{ formatDate(item.last_message_at) }}</td>
              <td>{{ formatDate(item.created_at) }}</td>
              <td>
                <RouterLink class="button small" :to="`/risks/${item.id}`">
                  查看处置 <ArrowUpRight :size="14" />
                </RouterLink>
              </td>
            </tr>
          </tbody>
        </table>
      </div>
      <div class="pagination">
        <span>共 {{ total }} 个会话 · 第 {{ page }} 页</span>
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

.risk-select {
  width: 170px;
}

.empty-state div {
  display: grid;
  justify-items: center;
  gap: 10px;
}
</style>
