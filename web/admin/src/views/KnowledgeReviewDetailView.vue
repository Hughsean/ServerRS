<script setup lang="ts">
import type { KnowledgeReviewDetail } from '@serverrs/sdk'
import { ArrowLeft, ExternalLink, Send } from '@lucide/vue'
import { computed, onMounted, ref } from 'vue'
import { useRoute } from 'vue-router'

import { api } from '@/lib/sdk'
import { errorMessage, formatDate, statusTone } from '@/utils/format'

const route = useRoute()
const id = computed(() => Number(route.params.id))
const detail = ref<KnowledgeReviewDetail | null>(null)
const loading = ref(false)
const publishing = ref(false)
const error = ref('')
const notes = ref('')
const riskFlagsText = ref('-')
const qualityResultText = ref('-')
const distilledText = ref('-')

async function load() {
  loading.value = true
  error.value = ''
  try {
    const result = await api.admin.knowledgeReview(id.value)
    detail.value = result
    riskFlagsText.value = result.review.risk_flags == null
      ? '-'
      : JSON.stringify(result.review.risk_flags, null, 2)
    qualityResultText.value = result.review.quality_result == null
      ? '-'
      : JSON.stringify(result.review.quality_result, null, 2)
    distilledText.value = result.distilled_json == null
      ? '-'
      : JSON.stringify(result.distilled_json, null, 2)
  } catch (cause) {
    error.value = errorMessage(cause)
  } finally {
    loading.value = false
  }
}

async function publish() {
  if (!detail.value || !window.confirm('确认该内容已经审核，可以提交发布吗？')) return
  publishing.value = true
  error.value = ''
  try {
    await api.admin.publishKnowledgeReview(id.value, notes.value || undefined)
    await load()
  } catch (cause) {
    error.value = errorMessage(cause)
  } finally {
    publishing.value = false
  }
}

onMounted(load)
</script>

<template>
  <div>
    <header class="page-heading">
      <div>
        <RouterLink class="back-link" to="/knowledge"><ArrowLeft :size="15" />返回审核队列</RouterLink>
        <h1>{{ detail?.review.title || `审核记录 #${id}` }}</h1>
        <p>核验抓取正文、蒸馏结构与审计日志。</p>
      </div>
      <a
        v-if="detail"
        class="button"
        :href="detail.review.source_url"
        rel="noopener noreferrer"
        target="_blank"
      >
        打开来源 <ExternalLink :size="15" />
      </a>
    </header>

    <div v-if="loading" class="card loading-state">正在加载审核详情...</div>
    <div v-else-if="error && !detail" class="card error-state">{{ error }}</div>
    <template v-else-if="detail">
      <p v-if="error" class="notice">{{ error }}</p>
      <section class="review-summary">
        <article class="card card-body">
          <dl class="detail-list">
            <div class="detail-row"><dt>发布记录</dt><dd>#{{ detail.review.publish_record_id }}</dd></div>
            <div class="detail-row"><dt>来源</dt><dd>{{ detail.review.source_name }} (#{{ detail.review.source_id }})</dd></div>
            <div class="detail-row"><dt>版本</dt><dd>{{ detail.review.version_key }}</dd></div>
            <div class="detail-row">
              <dt>状态</dt>
              <dd><span class="badge" :class="statusTone(detail.review.publish_status)">{{ detail.review.publish_status }}</span></dd>
            </div>
            <div class="detail-row"><dt>质量评分</dt><dd>{{ detail.review.quality_score ?? '-' }}</dd></div>
            <div class="detail-row"><dt>建议发布</dt><dd>{{ detail.review.should_publish == null ? '待判断' : detail.review.should_publish ? '是' : '否' }}</dd></div>
            <div class="detail-row"><dt>更新时间</dt><dd>{{ formatDate(detail.review.updated_at) }}</dd></div>
          </dl>
        </article>

        <article class="card publish-card">
          <div class="card-header"><h2>审核发布</h2></div>
          <div class="card-body publish-form">
            <p>提交后会创建发布事件，由后台任务继续完成知识入库。</p>
            <textarea v-model="notes" class="textarea" placeholder="填写审核意见（可选）"></textarea>
            <button
              class="button primary"
              :disabled="publishing || detail.review.publish_status !== 'staged'"
              @click="publish"
            >
              <Send :size="15" />
              {{ publishing ? '正在提交...' : detail.review.publish_status === 'staged' ? '审核通过并发布' : '当前状态不可发布' }}
            </button>
          </div>
        </article>
      </section>

      <section class="review-content-grid">
        <article class="card">
          <div class="card-header"><h2>清洗后正文</h2></div>
          <div class="document-text">{{ detail.clean_text || '暂无正文内容' }}</div>
        </article>
        <div class="review-side">
          <article class="card">
            <div class="card-header"><h2>风险标记</h2></div>
            <pre class="json-block">{{ riskFlagsText }}</pre>
          </article>
          <article class="card">
            <div class="card-header"><h2>质量结果</h2></div>
            <pre class="json-block">{{ qualityResultText }}</pre>
          </article>
          <article class="card">
            <div class="card-header"><h2>蒸馏结果</h2></div>
            <pre class="json-block">{{ distilledText }}</pre>
          </article>
        </div>
      </section>

      <section class="card audit-card">
        <div class="card-header"><h2>审计日志</h2></div>
        <div v-if="!detail.audit_logs.length" class="empty-state">暂无审计日志</div>
        <div v-else class="audit-list">
          <div v-for="(log, index) in detail.audit_logs" :key="`${log.created_at}-${index}`" class="audit-item">
            <span class="audit-dot"></span>
            <div>
              <div class="audit-title">
                <strong>{{ log.action }}</strong>
                <span class="badge" :class="statusTone(log.status)">{{ log.status }}</span>
                <time>{{ formatDate(log.created_at) }}</time>
              </div>
              <p>{{ log.message }}</p>
            </div>
          </div>
        </div>
      </section>
    </template>
  </div>
</template>

<style scoped>
.back-link {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  margin-bottom: 8px;
  color: var(--brand);
  font-size: 12px;
  font-weight: 700;
}

.notice {
  margin-bottom: 16px;
}

.review-summary {
  display: grid;
  grid-template-columns: minmax(0, 1.4fr) minmax(310px, 0.6fr);
  gap: 20px;
  margin-bottom: 20px;
}

.publish-form {
  display: grid;
  gap: 13px;
}

.publish-form p {
  color: var(--muted);
  font-size: 12px;
}

.review-content-grid {
  display: grid;
  grid-template-columns: minmax(0, 1.45fr) minmax(340px, 0.55fr);
  gap: 20px;
  align-items: start;
}

.review-side {
  display: grid;
  gap: 20px;
}

.document-text {
  max-height: 920px;
  overflow-y: auto;
  padding: 24px;
  line-height: 1.85;
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}

.json-block {
  max-height: 330px;
  margin: 0;
  overflow: auto;
  padding: 16px;
  color: #315349;
  background: #f6f9f7;
  font-family: "Cascadia Code", Consolas, monospace;
  font-size: 11px;
  line-height: 1.65;
  white-space: pre-wrap;
}

.audit-card {
  margin-top: 20px;
}

.audit-list {
  padding: 20px;
}

.audit-item {
  position: relative;
  display: grid;
  grid-template-columns: 16px 1fr;
  gap: 10px;
  padding-bottom: 20px;
}

.audit-item:not(:last-child)::before {
  position: absolute;
  top: 13px;
  bottom: 0;
  left: 5px;
  width: 1px;
  background: var(--line);
  content: "";
}

.audit-dot {
  position: relative;
  z-index: 1;
  width: 11px;
  height: 11px;
  margin-top: 5px;
  border: 3px solid var(--brand-soft);
  border-radius: 50%;
  background: var(--brand);
}

.audit-title {
  display: flex;
  align-items: center;
  gap: 9px;
}

.audit-title time {
  margin-left: auto;
  color: var(--muted);
  font-size: 11px;
}

.audit-item p {
  margin-top: 5px;
  color: var(--muted);
}

@media (max-width: 1050px) {
  .review-summary,
  .review-content-grid {
    grid-template-columns: 1fr;
  }
}
</style>
