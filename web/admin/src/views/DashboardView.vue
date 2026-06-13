<script setup lang="ts">
import type { KnowledgeReview } from '@serverrs/sdk'
import { BookOpenCheck, Music2, ShieldAlert, Users } from '@lucide/vue'
import { onMounted, ref } from 'vue'

import { api } from '@/lib/sdk'
import { errorMessage, formatDate, formatNumber, statusTone } from '@/utils/format'

const loading = ref(true)
const error = ref('')
const totals = ref({ users: 0, risks: 0, reviews: 0, music: 0 })
const recentReviews = ref<KnowledgeReview[]>([])

async function load() {
  loading.value = true
  error.value = ''
  const [users, risks, reviews, music] = await Promise.allSettled([
    api.admin.users({ page: 1, pageSize: 1 }),
    api.admin.riskConversations({ page: 1, pageSize: 1 }),
    api.admin.knowledgeReviews({ page: 1, pageSize: 5 }),
    api.music.tracks({ page: 1, pageSize: 1 }),
  ])

  if (users.status === 'fulfilled') totals.value.users = users.value.total
  if (risks.status === 'fulfilled') totals.value.risks = risks.value.total
  if (reviews.status === 'fulfilled') {
    totals.value.reviews = reviews.value.total
    recentReviews.value = reviews.value.items
  }
  if (music.status === 'fulfilled') totals.value.music = music.value.total
  const rejected = [users, risks, reviews, music].find((item) => item.status === 'rejected')
  if (rejected?.status === 'rejected') error.value = errorMessage(rejected.reason)
  loading.value = false
}

onMounted(load)
</script>

<template>
  <div>
    <header class="page-heading">
      <div>
        <h1>运行概览</h1>
        <p>关键业务数据与待处理事项的实时入口。</p>
      </div>
      <button class="button" :disabled="loading" @click="load">刷新数据</button>
    </header>

    <p v-if="error" class="notice">{{ error }}，部分数据可能未加载。</p>

    <section class="stats-grid">
      <article class="card stat-card">
        <div class="stat-icon"><Users :size="19" /></div>
        <strong>{{ loading ? '—' : formatNumber(totals.users) }}</strong>
        <span>系统用户总数</span>
      </article>
      <article class="card stat-card">
        <div class="stat-icon"><ShieldAlert :size="19" /></div>
        <strong>{{ loading ? '—' : formatNumber(totals.risks) }}</strong>
        <span>风险相关会话</span>
      </article>
      <article class="card stat-card">
        <div class="stat-icon"><BookOpenCheck :size="19" /></div>
        <strong>{{ loading ? '—' : formatNumber(totals.reviews) }}</strong>
        <span>知识审核记录</span>
      </article>
      <article class="card stat-card">
        <div class="stat-icon"><Music2 :size="19" /></div>
        <strong>{{ loading ? '—' : formatNumber(totals.music) }}</strong>
        <span>音乐资源总数</span>
      </article>
    </section>

    <section class="content-grid">
      <article class="card">
        <div class="card-header">
          <h2>最近知识审核</h2>
          <RouterLink class="button small ghost" to="/knowledge">查看全部</RouterLink>
        </div>
        <div v-if="loading" class="loading-state">正在汇总数据...</div>
        <div v-else-if="!recentReviews.length" class="empty-state">暂无审核记录</div>
        <div v-else class="table-wrap">
          <table class="data-table">
            <thead>
              <tr><th>文档</th><th>来源</th><th>状态</th><th>更新时间</th></tr>
            </thead>
            <tbody>
              <tr v-for="item in recentReviews" :key="item.publish_record_id">
                <td>
                  <RouterLink class="cell-title" :to="`/knowledge/${item.publish_record_id}`">
                    <strong>{{ item.title || `文档 #${item.document_id}` }}</strong>
                    <span>{{ item.source_url }}</span>
                  </RouterLink>
                </td>
                <td>{{ item.source_name }}</td>
                <td>
                  <span class="badge" :class="statusTone(item.publish_status)">
                    {{ item.publish_status }}
                  </span>
                </td>
                <td>{{ formatDate(item.updated_at) }}</td>
              </tr>
            </tbody>
          </table>
        </div>
      </article>

      <article class="card quick-card">
        <div class="card-header"><h2>快速入口</h2></div>
        <div class="card-body quick-links">
          <RouterLink to="/knowledge">
            <BookOpenCheck :size="20" />
            <div><strong>审核知识</strong><span>检查质量与风险标记</span></div>
          </RouterLink>
          <RouterLink to="/risks">
            <ShieldAlert :size="20" />
            <div><strong>处理风险</strong><span>查看高风险对话内容</span></div>
          </RouterLink>
          <RouterLink to="/users">
            <Users :size="20" />
            <div><strong>管理用户</strong><span>调整账号状态和角色</span></div>
          </RouterLink>
        </div>
      </article>
    </section>
  </div>
</template>

<style scoped>
.notice {
  margin-bottom: 18px;
}

.quick-links {
  display: grid;
  gap: 10px;
}

.quick-links a {
  display: flex;
  align-items: center;
  gap: 13px;
  padding: 13px;
  border: 1px solid var(--line);
  border-radius: 11px;
}

.quick-links a:hover {
  border-color: #aec1ba;
  background: #f8fbfa;
}

.quick-links svg {
  color: var(--brand);
}

.quick-links div {
  display: flex;
  flex-direction: column;
}

.quick-links span {
  color: var(--muted);
  font-size: 11px;
}
</style>
