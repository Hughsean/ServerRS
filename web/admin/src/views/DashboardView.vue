<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { use } from 'echarts/core'
import { LineChart, PieChart } from 'echarts/charts'
import { GridComponent, TooltipComponent } from 'echarts/components'
import { CanvasRenderer } from 'echarts/renderers'
import VChart from 'vue-echarts'
import type { KnowledgeReview } from '@serverrs/sdk'
import { BookOpenCheck, Music2, ShieldAlert, Users, ArrowUpRight } from '@lucide/vue'
import { RouterLink } from 'vue-router'
import { api } from '@/lib/sdk'
import { formatDate, formatNumber, errorMessage } from '@/utils/format'

// 按需注册 ECharts 组件
use([LineChart, PieChart, GridComponent, TooltipComponent, CanvasRenderer])

const loading = ref(true)
const error = ref('')
const totals = ref({ users: 0, risks: 0, reviews: 0, music: 0 })
const recentReviews = ref<KnowledgeReview[]>([])

/* ── ECharts 配置 ── */

const sparklineOption = (data: number[]) => ({
  grid: { show: false, left: 0, right: 0, top: 4, bottom: 0 },
  xAxis: { show: false },
  yAxis: { show: false },
  series: [{
    type: 'line' as const, data, smooth: true,
    lineStyle: { width: 2, color: '#156354' },
    areaStyle: { color: 'rgba(21, 99, 84, 0.08)' },
    showSymbol: false,
  }],
})

const trendOption = {
  tooltip: { trigger: 'axis' as const },
  grid: { left: 40, right: 16, top: 24, bottom: 24 },
  xAxis: { type: 'category' as const, data: ['06-19', '06-20', '06-21', '06-22', '06-23', '06-24', '06-25'], axisLabel: { fontSize: 11, color: '#63756f' } },
  yAxis: { type: 'value' as const, min: 0, splitLine: { lineStyle: { color: 'rgba(190,205,200,0.3)' } } },
  series: [{
    type: 'line' as const, data: [3, 7, 5, 9, 4, 8, 6], smooth: true,
    lineStyle: { width: 2.5, color: '#156354' },
    areaStyle: { color: 'rgba(21, 99, 84, 0.06)' },
    showSymbol: true, symbol: 'circle', symbolSize: 6,
    itemStyle: { color: '#156354' },
  }],
}

const pieOption = {
  tooltip: { trigger: 'item' as const, formatter: '{b}: {c} ({d}%)' },
  series: [{
    type: 'pie' as const, radius: ['42%', '68%'],
    data: [
      { value: 20, name: 'none', itemStyle: { color: '#8ba098' } },
      { value: 8, name: 'low', itemStyle: { color: '#d8913a' } },
      { value: 12, name: 'medium', itemStyle: { color: '#e8b45e' } },
      { value: 5, name: 'high', itemStyle: { color: '#b94242' } },
      { value: 2, name: 'critical', itemStyle: { color: '#8b1a1a' } },
    ],
    label: { show: true, color: '#63756f', fontSize: 11, formatter: '{b}: {d}%' },
    emphasis: { scale: false },
    labelLine: { lineStyle: { color: 'rgba(190,205,200,0.5)' } },
  }],
}

/* ── 数据 ── */

const sparklineData = {
  users: [1240, 1255, 1261, 1270, 1278, 1282, 1284],
  risks: [52, 50, 53, 49, 48, 47, 47],
  reviews: [284, 291, 299, 303, 308, 310, 312],
  music: [83, 84, 85, 86, 87, 88, 89],
}

async function load() {
  loading.value = true
  error.value = ''
  const [users, risks, reviews, music] = await Promise.allSettled([
    api.admin.users({ page: 1, pageSize: 1 }),
    api.admin.riskConversations({ page: 1, pageSize: 1 }),
    api.admin.knowledgeReviews({ page: 1, pageSize: 5 }),
    api.admin.tracks({ page: 1, pageSize: 1 }),
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

    <!-- 统计卡 + 迷你趋势图 -->
    <section class="stats-grid">
      <article class="card stat-card">
        <div class="stat-icon"><Users :size="19" /></div>
        <strong>{{ loading ? '—' : formatNumber(totals.users) }}</strong>
        <span>系统用户总数 · 本周 +12</span>
        <VChart v-if="!loading" class="sparkline" :option="sparklineOption(sparklineData.users)" autoresize />
      </article>
      <article class="card stat-card">
        <div class="stat-icon"><ShieldAlert :size="19" /></div>
        <strong>{{ loading ? '—' : formatNumber(totals.risks) }}</strong>
        <span>风险相关会话 · 本周 -3</span>
        <VChart v-if="!loading" class="sparkline" :option="sparklineOption(sparklineData.risks)" autoresize />
      </article>
      <article class="card stat-card">
        <div class="stat-icon"><BookOpenCheck :size="19" /></div>
        <strong>{{ loading ? '—' : formatNumber(totals.reviews) }}</strong>
        <span>知识审核记录 · 本周 +28</span>
        <VChart v-if="!loading" class="sparkline" :option="sparklineOption(sparklineData.reviews)" autoresize />
      </article>
      <article class="card stat-card">
        <div class="stat-icon"><Music2 :size="19" /></div>
        <strong>{{ loading ? '—' : formatNumber(totals.music) }}</strong>
        <span>音乐资源总数 · 本周 +2</span>
        <VChart v-if="!loading" class="sparkline" :option="sparklineOption(sparklineData.music)" autoresize />
      </article>
    </section>

    <!-- 图表区域 -->
    <section class="chart-grid">
      <article class="card">
        <div class="card-header"><h2>风险等级分布</h2></div>
        <div class="card-body chart-body">
          <VChart v-if="!loading" class="chart" :option="pieOption" autoresize />
        </div>
      </article>
      <article class="card">
        <div class="card-header"><h2>近 7 天风险趋势</h2></div>
        <div class="card-body chart-body">
          <VChart v-if="!loading" class="chart" :option="trendOption" autoresize />
        </div>
      </article>
    </section>

    <!-- 最近知识审核 -->
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
                  <span class="badge" :class="item.publish_status === 'published' ? 'success' : item.publish_status === 'failed' ? 'danger' : ''">
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

.sparkline {
  height: 48px;
  margin-top: 8px;
}

.chart-grid {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 16px;
  margin-bottom: 20px;
}

.chart-body {
  padding: 8px;
}

.chart {
  height: 260px;
  width: 100%;
}

.stats-grid {
  display: grid;
  grid-template-columns: repeat(4, minmax(0, 1fr));
  gap: 16px;
  margin-bottom: 20px;
}

.stat-card {
  position: relative;
  min-height: 138px;
  overflow: hidden;
  padding: 20px;
}

.stat-card::after {
  position: absolute;
  right: -26px;
  bottom: -42px;
  width: 110px;
  height: 110px;
  border-radius: 50%;
  background: var(--brand-soft);
  content: "";
}

.stat-icon {
  display: grid;
  width: 38px;
  height: 38px;
  place-items: center;
  border-radius: 11px;
  color: var(--brand);
  background: var(--brand-soft);
}

.stat-card strong {
  display: block;
  margin-top: 14px;
  font-family: Georgia, serif;
  font-size: 28px;
  line-height: 1;
}

.stat-card span {
  display: block;
  margin-top: 7px;
  color: var(--muted);
  font-size: 12px;
}

.content-grid {
  display: grid;
  grid-template-columns: minmax(0, 1.65fr) minmax(300px, 0.8fr);
  gap: 20px;
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

@media (max-width: 1100px) {
  .stats-grid {
    grid-template-columns: repeat(2, minmax(0, 1fr));
  }
  .chart-grid {
    grid-template-columns: 1fr;
  }
  .content-grid {
    grid-template-columns: 1fr;
  }
}

@media (max-width: 760px) {
  .stats-grid {
    grid-template-columns: 1fr;
  }
}
</style>
