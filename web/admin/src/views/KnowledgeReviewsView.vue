<script setup lang="ts">
import type { KnowledgeReview } from '@serverrs/sdk'
import { ArrowUpRight, BookOpenCheck, Send } from '@lucide/vue'
import { onMounted, ref } from 'vue'

import { api } from '@/lib/sdk'
import { errorMessage, formatDate, statusTone } from '@/utils/format'

const items = ref<KnowledgeReview[]>([])
const loading = ref(false)
const publishingId = ref<number | null>(null)
const error = ref('')
const status = ref('staged')
const sourceId = ref('')
const page = ref(1)
const pageSize = 20
const total = ref(0)

async function load(reset = false) {
  if (reset) page.value = 1
  loading.value = true
  error.value = ''
  try {
    const result = await api.admin.knowledgeReviews({
      page: page.value,
      pageSize,
      status: status.value || undefined,
      sourceId: sourceId.value ? Number(sourceId.value) : undefined,
    })
    items.value = result.items
    total.value = result.total
  } catch (cause) {
    error.value = errorMessage(cause)
  } finally {
    loading.value = false
  }
}

async function requestPublish(publishRecordId: number, title: string) {
  if (!window.confirm(`确认将“${title}”提交发布吗？`)) return
  publishingId.value = publishRecordId
  try {
    await api.admin.publishKnowledgeReview(publishRecordId, '通过管理后台审核发布')
    await load()
  } catch (cause) {
    error.value = errorMessage(cause)
  } finally {
    publishingId.value = null
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
        <h1>知识审核</h1>
        <p>检查抓取内容、质量评分、风险标记及蒸馏结果，再进入发布流程。</p>
      </div>
      <div class="toolbar">
        <select v-model="status" class="select filter-select" @change="load(true)">
          <option value="">全部状态</option>
          <option value="staged">staged</option>
          <option value="published">published</option>
          <option value="superseded">superseded</option>
          <option value="rolled_back">rolled_back</option>
          <option value="failed">failed</option>
        </select>
        <input
          v-model="sourceId"
          class="input source-input"
          inputmode="numeric"
          placeholder="来源 ID"
          @keyup.enter="load(true)"
        />
        <button class="button" :disabled="loading" @click="load(true)">筛选</button>
      </div>
    </header>

    <p v-if="error" class="notice">{{ error }}</p>

    <section class="card">
      <div v-if="loading" class="loading-state">正在加载审核队列...</div>
      <div v-else-if="!items.length" class="empty-state">
        <div><BookOpenCheck :size="34" /><p>当前筛选条件下没有审核记录</p></div>
      </div>
      <div v-else class="table-wrap">
        <table class="data-table">
          <thead>
            <tr><th>文档</th><th>来源</th><th>质量</th><th>建议发布</th><th>状态</th><th>更新时间</th><th>操作</th></tr>
          </thead>
          <tbody>
            <tr v-for="item in items" :key="item.publish_record_id">
              <td>
                <RouterLink class="cell-title" :to="`/knowledge/${item.publish_record_id}`">
                  <strong>{{ item.title || `文档 #${item.document_id}` }}</strong>
                  <span>{{ item.source_url }}</span>
                </RouterLink>
              </td>
              <td>
                <div class="cell-title">
                  <strong>{{ item.source_name }}</strong>
                  <span>Source #{{ item.source_id }}</span>
                </div>
              </td>
              <td>{{ item.quality_score == null ? '-' : item.quality_score.toFixed(2) }}</td>
              <td>
                <span
                  class="badge"
                  :class="item.should_publish === true ? 'success' : item.should_publish === false ? 'danger' : ''"
                >
                  {{ item.should_publish == null ? '待判断' : item.should_publish ? '是' : '否' }}
                </span>
              </td>
              <td><span class="badge" :class="statusTone(item.publish_status)">{{ item.publish_status }}</span></td>
              <td>{{ formatDate(item.updated_at) }}</td>
              <td>
                <div class="row-actions">
                  <RouterLink class="button small" :to="`/knowledge/${item.publish_record_id}`">
                    详情 <ArrowUpRight :size="14" />
                  </RouterLink>
                  <button
                    v-if="item.publish_status === 'staged'"
                    class="button small primary"
                    :disabled="publishingId === item.publish_record_id"
                    @click="
                      requestPublish(
                        item.publish_record_id,
                        item.title || item.source_url,
                      )
                    "
                  >
                    <Send :size="13" />发布
                  </button>
                </div>
              </td>
            </tr>
          </tbody>
        </table>
      </div>
      <div class="pagination">
        <span>共 {{ total }} 条记录 · 第 {{ page }} 页</span>
        <button class="button small" :disabled="page <= 1 || loading" @click="turnPage(page - 1)">上一页</button>
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

.filter-select {
  width: 155px;
}

.source-input {
  width: 110px;
}

.row-actions {
  display: flex;
  gap: 7px;
}

.empty-state div {
  display: grid;
  justify-items: center;
  gap: 10px;
}
</style>
