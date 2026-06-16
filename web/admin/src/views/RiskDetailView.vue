<script setup lang="ts">
import type { AdminRiskConversationDetail, RiskAuditAdminDto } from '@serverrs/sdk'
import { ArrowLeft, ShieldAlert } from '@lucide/vue'
import { computed, onMounted, ref } from 'vue'
import { useRoute } from 'vue-router'

import { api } from '@/lib/sdk'
import { errorMessage, formatDate, statusTone } from '@/utils/format'

const route = useRoute()
const id = computed(() => Number(route.params.id))
const detail = ref<AdminRiskConversationDetail | null>(null)
const loading = ref(false)
const error = ref('')

async function load() {
  loading.value = true
  error.value = ''
  try {
    detail.value = await api.admin.riskConversation(id.value)
  } catch (cause) {
    error.value = errorMessage(cause)
  } finally {
    loading.value = false
  }
}

onMounted(load)
</script>

<template>
  <div>
    <header class="page-heading">
      <div>
        <RouterLink class="back-link" to="/risks"><ArrowLeft :size="15" />返回风险会话</RouterLink>
        <h1>{{ detail?.conversation.title || `会话 #${id}` }}</h1>
        <p>完整消息上下文与风险检测记录。</p>
      </div>
      <button class="button" :disabled="loading" @click="load">刷新</button>
    </header>

    <div v-if="loading" class="card loading-state">正在加载会话详情...</div>
    <div v-else-if="error && !detail" class="card error-state">{{ error }}</div>
    <template v-else-if="detail">
      <p v-if="error" class="notice">{{ error }}</p>
      <section class="risk-layout">
        <article class="card">
          <div class="card-header">
            <h2>对话消息</h2>
            <span class="badge">{{ detail.messages.length }} 条</span>
          </div>
          <div class="message-list">
            <div
              v-for="message in detail.messages"
              :key="message.id"
              class="message"
              :class="message.sender_role"
            >
              <div class="message-meta">
                <strong>{{ message.sender_role }}</strong>
                <span>{{ formatDate(message.created_at) }}</span>
              </div>
              <p>{{ message.content }}</p>
            </div>
            <div v-if="!detail.messages.length" class="empty-state">该会话暂无消息</div>
          </div>
        </article>

        <aside class="detections">
          <article
            v-for="item in detail.risk_audits"
            :key="item.audit_id"
            class="card detection-card"
          >
            <div class="detection-heading">
              <div class="stat-icon"><ShieldAlert :size="18" /></div>
              <div>
                <span class="badge" :class="statusTone(item.risk_level ?? 'none')">{{ item.risk_level || 'none' }}</span>
                <strong>审计 #{{ item.audit_id }}</strong>
              </div>
            </div>
            <dl class="detail-list">
              <div class="detail-row"><dt>检测范围</dt><dd>{{ item.audit_scope }}</dd></div>
              <div class="detail-row"><dt>检测器</dt><dd>{{ item.detector_name || '-' }}</dd></div>
              <div class="detail-row"><dt>状态</dt><dd>{{ item.status }}</dd></div>
              <div class="detail-row"><dt>置信度</dt><dd>{{ item.confidence != null ? Math.round(item.confidence * 100) + '%' : '-' }}</dd></div>
              <div class="detail-row"><dt>错误信息</dt><dd>{{ item.error_message || '-' }}</dd></div>
              <div class="detail-row"><dt>来源已删除</dt><dd>{{ item.source_deleted ? '是' : '否' }}</dd></div>
              <div class="detail-row"><dt>检测时间</dt><dd>{{ formatDate(item.created_at) }}</dd></div>
            </dl>
          </article>
          <div v-if="!detail.risk_audits.length" class="card empty-state">暂无风险审计记录</div>
        </aside>
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

.risk-layout {
  display: grid;
  grid-template-columns: minmax(0, 1.35fr) minmax(320px, 0.75fr);
  gap: 20px;
  align-items: start;
}

.message-list {
  display: grid;
  gap: 14px;
  padding: 20px;
}

.message {
  max-width: 86%;
  padding: 13px 15px;
  border: 1px solid var(--line);
  border-radius: 4px 14px 14px 14px;
  background: #f7faf8;
}

.message.assistant {
  justify-self: end;
  border-radius: 14px 4px 14px 14px;
  background: var(--brand-soft);
}

.message.system,
.message.tool {
  max-width: 100%;
  border-style: dashed;
  background: #fff7ea;
}

.message-meta {
  display: flex;
  justify-content: space-between;
  gap: 14px;
  margin-bottom: 7px;
  font-size: 11px;
}

.message-meta span {
  color: var(--muted);
}

.message p {
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}

.detections {
  display: grid;
  gap: 16px;
}

.detection-card {
  padding: 18px;
}

.detection-heading {
  display: flex;
  align-items: center;
  gap: 11px;
  margin-bottom: 12px;
}

.detection-heading > div:last-child {
  display: flex;
  flex: 1;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
}

.detail-row {
  grid-template-columns: 92px minmax(0, 1fr);
  gap: 12px;
  font-size: 12px;
}

.processed-box {
  display: flex;
  gap: 9px;
  margin-top: 16px;
  padding: 12px;
  border-radius: 10px;
  color: var(--success);
  background: var(--success-soft);
}

.processed-box p {
  margin-top: 2px;
  color: #4f6e65;
  font-size: 11px;
}

.process-form {
  display: grid;
  gap: 10px;
  margin-top: 16px;
}

@media (max-width: 1050px) {
  .risk-layout {
    grid-template-columns: 1fr;
  }
}
</style>
