<script setup lang="ts">
import type {
  Article,
  Category,
  PsychologyResource,
  Qna,
} from '@serverrs/sdk'
import { BookOpen, Pencil, Plus, Trash2, X } from '@lucide/vue'
import { computed, onMounted, reactive, ref, shallowRef } from 'vue'

import { api } from '@/lib/sdk'
import { errorMessage } from '@/utils/format'
import { useToast } from '@/utils/toast'
import ConfirmDialog from '@/components/ConfirmDialog.vue'

type ContentTab = 'categories' | 'articles' | 'qna' | 'resources'

const activeTab = ref<ContentTab>('categories')
const categories = shallowRef<Category[]>([])
const articles = shallowRef<Article[]>([])
const qnas = shallowRef<Qna[]>([])
const resources = shallowRef<PsychologyResource[]>([])
const loading = ref(false)
const saving = ref(false)
const error = ref('')
const page = ref(1)
const pageSize = 20
const total = ref(0)
const publishedFilter = ref<'' | 'true' | 'false'>('')
const categoryFilter = ref('')
const showEditor = ref(false)
const editingId = ref<number | null>(null)
const confirmRemove = ref<{ id: number; label: string } | null>(null)
const toast = useToast()

const form = reactive({
  parentId: '',
  name: '',
  description: '',
  sortOrder: 0,
  isEnabled: true,
  categoryId: '',
  title: '',
  summary: '',
  content: '',
  author: '',
  source: '',
  isFeatured: false,
  question: '',
  answer: '',
  expertName: '',
  expertTitle: '',
  isVerified: false,
  resourceType: 'LINK' as 'VIDEO' | 'AUDIO' | 'PDF' | 'LINK' | 'TOOL',
  externalUrl: '',
  tags: '',
  isPublished: true,
})

const tabTitle = computed(() => ({
  categories: '分类',
  articles: '文章',
  qna: '问答',
  resources: '资源',
})[activeTab.value])

function publicationFilter(): boolean | undefined {
  return publishedFilter.value === '' ? undefined : publishedFilter.value === 'true'
}

async function load(reset = false) {
  if (reset) page.value = 1
  loading.value = true
  error.value = ''
  try {
    categories.value = await api.admin.psychologyCategories()
    const query = {
      page: page.value,
      pageSize,
      categoryId: categoryFilter.value ? Number(categoryFilter.value) : undefined,
      isPublished: publicationFilter(),
    }
    if (activeTab.value === 'articles') {
      const result = await api.admin.psychologyArticles(query)
      articles.value = result.items
      total.value = result.total
    } else if (activeTab.value === 'qna') {
      const result = await api.admin.psychologyQna(query)
      qnas.value = result.items
      total.value = result.total
    } else if (activeTab.value === 'resources') {
      const result = await api.admin.psychologyResources(query)
      resources.value = result.items
      total.value = result.total
    } else {
      total.value = categories.value.length
    }
  } catch (cause) {
    error.value = errorMessage(cause)
  } finally {
    loading.value = false
  }
}

async function switchTab(tab: ContentTab) {
  activeTab.value = tab
  publishedFilter.value = ''
  categoryFilter.value = ''
  await load(true)
}

function resetForm() {
  Object.assign(form, {
    parentId: '',
    name: '',
    description: '',
    sortOrder: 0,
    isEnabled: true,
    categoryId: categories.value[0]?.categoryId ? String(categories.value[0].categoryId) : '',
    title: '',
    summary: '',
    content: '',
    author: '',
    source: '',
    isFeatured: false,
    question: '',
    answer: '',
    expertName: '',
    expertTitle: '',
    isVerified: false,
    resourceType: 'LINK',
    externalUrl: '',
    tags: '',
    isPublished: true,
  })
}

function openCreate() {
  editingId.value = null
  resetForm()
  showEditor.value = true
}

function tagsForInput(raw: string | null): string {
  if (!raw) return ''
  try {
    const value = JSON.parse(raw)
    return Array.isArray(value) ? value.join(', ') : ''
  } catch {
    return ''
  }
}

function openCategory(category: Category) {
  editingId.value = category.categoryId
  resetForm()
  Object.assign(form, {
    parentId: category.parentId == null ? '' : String(category.parentId),
    name: category.categoryName,
    description: category.description ?? '',
    sortOrder: category.sortOrder,
    isEnabled: category.isEnabled,
  })
  showEditor.value = true
}

function openArticle(article: Article) {
  editingId.value = article.articleId
  resetForm()
  Object.assign(form, {
    categoryId: String(article.categoryId ?? ''),
    title: article.title,
    summary: article.summary ?? '',
    content: article.content,
    author: article.author ?? '',
    source: article.source ?? '',
    isFeatured: article.isFeatured,
    tags: tagsForInput(article.tags),
    isPublished: article.isPublished,
  })
  showEditor.value = true
}

function openQna(qna: Qna) {
  editingId.value = qna.qnaId
  resetForm()
  Object.assign(form, {
    categoryId: String(qna.categoryId ?? ''),
    question: qna.question,
    answer: qna.answer,
    expertName: qna.expertName ?? '',
    expertTitle: qna.expertTitle ?? '',
    isVerified: qna.isVerified,
    tags: tagsForInput(qna.tags),
    isPublished: qna.isPublished,
  })
  showEditor.value = true
}

function openResource(resource: PsychologyResource) {
  editingId.value = resource.resourceId
  resetForm()
  Object.assign(form, {
    categoryId: String(resource.categoryId ?? ''),
    title: resource.title,
    description: resource.description ?? '',
    resourceType: resource.resourceType,
    externalUrl: resource.externalUrl ?? '',
    tags: tagsForInput(resource.tags),
    isPublished: resource.isPublished,
  })
  showEditor.value = true
}

function parsedTags(): string[] | undefined {
  const tags = form.tags.split(',').map((tag) => tag.trim()).filter(Boolean)
  return tags.length ? tags : undefined
}

async function save() {
  saving.value = true
  error.value = ''
  try {
    if (activeTab.value === 'categories') {
      const payload = {
        parentId: form.parentId ? Number(form.parentId) : undefined,
        name: form.name,
        description: form.description || undefined,
        sortOrder: form.sortOrder,
        isEnabled: form.isEnabled,
      }
      if (editingId.value == null) await api.admin.createPsychologyCategory(payload)
      else await api.admin.updatePsychologyCategory(editingId.value, payload)
    } else if (activeTab.value === 'articles') {
      const payload = {
        categoryId: Number(form.categoryId),
        title: form.title,
        summary: form.summary || undefined,
        content: form.content,
        author: form.author || undefined,
        source: form.source || undefined,
        tags: parsedTags(),
        isFeatured: form.isFeatured,
        isPublished: form.isPublished,
      }
      if (editingId.value == null) await api.admin.createPsychologyArticle(payload)
      else await api.admin.updatePsychologyArticle(editingId.value, payload)
    } else if (activeTab.value === 'qna') {
      const payload = {
        categoryId: Number(form.categoryId),
        question: form.question,
        answer: form.answer,
        expertName: form.expertName || undefined,
        expertTitle: form.expertTitle || undefined,
        tags: parsedTags(),
        isVerified: form.isVerified,
        isPublished: form.isPublished,
      }
      if (editingId.value == null) await api.admin.createPsychologyQna(payload)
      else await api.admin.updatePsychologyQna(editingId.value, payload)
    } else {
      const payload = {
        categoryId: Number(form.categoryId),
        title: form.title,
        description: form.description || undefined,
        resourceType: form.resourceType,
        externalUrl: form.externalUrl || undefined,
        tags: parsedTags(),
        isPublished: form.isPublished,
      }
      if (editingId.value == null) await api.admin.createPsychologyResource(payload)
      else await api.admin.updatePsychologyResource(editingId.value, payload)
    }
    showEditor.value = false
    await load()
  } catch (cause) {
    error.value = errorMessage(cause)
  } finally {
    saving.value = false
  }
}

async function confirmRemoveItem(id: number, label: string) {
  confirmRemove.value = { id, label }
}

async function remove() {
  const target = confirmRemove.value
  if (!target) return
  saving.value = true
  error.value = ''
  try {
    if (activeTab.value === 'categories') await api.admin.deletePsychologyCategory(target.id)
    else if (activeTab.value === 'articles') await api.admin.deletePsychologyArticle(target.id)
    else if (activeTab.value === 'qna') await api.admin.deletePsychologyQna(target.id)
    else await api.admin.deletePsychologyResource(target.id)
    confirmRemove.value = null
    toast.success(`“${target.label}”已删除`)
    await load()
  } catch (cause) {
    error.value = errorMessage(cause)
  } finally {
    saving.value = false
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
        <h1>心理内容</h1>
        <p>管理普通用户端可见的分类、文章、问答与外部资源。</p>
      </div>
      <button class="button primary" @click="openCreate"><Plus :size="16" />新增{{ tabTitle }}</button>
    </header>

    <div class="tabs">
      <button v-for="tab in (['categories', 'articles', 'qna', 'resources'] as ContentTab[])"
        :key="tab" class="tab" :class="{ active: activeTab === tab }" @click="switchTab(tab)">
        {{ { categories: '分类', articles: '文章', qna: '问答', resources: '资源' }[tab] }}
      </button>
    </div>

    <p v-if="error" class="notice">{{ error }}</p>

    <section class="card">
      <div v-if="activeTab !== 'categories'" class="filter-bar">
        <select v-model="categoryFilter" class="select" @change="load(true)">
          <option value="">全部分类</option>
          <option v-for="category in categories" :key="category.categoryId" :value="category.categoryId">
            {{ category.categoryName }}
          </option>
        </select>
        <select v-model="publishedFilter" class="select" @change="load(true)">
          <option value="">全部状态</option>
          <option value="true">已发布</option>
          <option value="false">草稿/隐藏</option>
        </select>
      </div>

      <div v-if="loading" class="loading-state">正在加载内容...</div>
      <div v-else-if="activeTab === 'categories' && !categories.length" class="empty-state">
        <div><BookOpen :size="34" /><p>暂无分类</p></div>
      </div>
      <div v-else class="table-wrap">
        <table v-if="activeTab === 'categories'" class="data-table">
          <thead><tr><th>分类</th><th>父分类</th><th>排序</th><th>状态</th><th>操作</th></tr></thead>
          <tbody>
            <tr v-for="item in categories" :key="item.categoryId">
              <td><div class="cell-title"><strong>{{ item.categoryName }}</strong><span>{{ item.description || `#${item.categoryId}` }}</span></div></td>
              <td>{{ categories.find((category) => category.categoryId === item.parentId)?.categoryName || '-' }}</td>
              <td>{{ item.sortOrder }}</td>
              <td><span class="badge">{{ item.isEnabled ? '启用' : '停用' }}</span></td>
              <td><div class="row-actions">
                <button class="button small" @click="openCategory(item)"><Pencil :size="13" />编辑</button>
                <button class="button small danger" @click="confirmRemoveItem(item.categoryId, item.categoryName)"><Trash2 :size="13" />删除</button>
              </div></td>
            </tr>
          </tbody>
        </table>

        <table v-else-if="activeTab === 'articles'" class="data-table">
          <thead><tr><th>文章</th><th>分类</th><th>状态</th><th>浏览/点赞</th><th>操作</th></tr></thead>
          <tbody>
            <tr v-for="item in articles" :key="item.articleId">
              <td><div class="cell-title"><strong>{{ item.title }}</strong><span>{{ item.summary || `#${item.articleId}` }}</span></div></td>
              <td>{{ categories.find((category) => category.categoryId === item.categoryId)?.categoryName || '-' }}</td>
              <td><span class="badge">{{ item.isPublished ? '已发布' : '草稿' }}</span></td>
              <td>{{ item.viewCount }} / {{ item.likeCount }}</td>
              <td><div class="row-actions">
                <button class="button small" @click="openArticle(item)"><Pencil :size="13" />编辑</button>
                <button class="button small danger" @click="confirmRemoveItem(item.articleId, item.title)"><Trash2 :size="13" />删除</button>
              </div></td>
            </tr>
          </tbody>
        </table>

        <table v-else-if="activeTab === 'qna'" class="data-table">
          <thead><tr><th>问题</th><th>分类</th><th>状态</th><th>审核</th><th>操作</th></tr></thead>
          <tbody>
            <tr v-for="item in qnas" :key="item.qnaId">
              <td><div class="cell-title"><strong>{{ item.question }}</strong><span>#{{ item.qnaId }}</span></div></td>
              <td>{{ categories.find((category) => category.categoryId === item.categoryId)?.categoryName || '-' }}</td>
              <td><span class="badge">{{ item.isPublished ? '已发布' : '隐藏' }}</span></td>
              <td>{{ item.isVerified ? '已验证' : '未验证' }}</td>
              <td><div class="row-actions">
                <button class="button small" @click="openQna(item)"><Pencil :size="13" />编辑</button>
                <button class="button small danger" @click="confirmRemoveItem(item.qnaId, item.question)"><Trash2 :size="13" />删除</button>
              </div></td>
            </tr>
          </tbody>
        </table>

        <table v-else class="data-table">
          <thead><tr><th>资源</th><th>类型</th><th>分类</th><th>状态</th><th>操作</th></tr></thead>
          <tbody>
            <tr v-for="item in resources" :key="item.resourceId">
              <td><div class="cell-title"><strong>{{ item.title }}</strong><span>{{ item.externalUrl || `#${item.resourceId}` }}</span></div></td>
              <td><span class="badge">{{ item.resourceType }}</span></td>
              <td>{{ categories.find((category) => category.categoryId === item.categoryId)?.categoryName || '-' }}</td>
              <td><span class="badge">{{ item.isPublished ? '已发布' : '停用' }}</span></td>
              <td><div class="row-actions">
                <button class="button small" @click="openResource(item)"><Pencil :size="13" />编辑</button>
                <button class="button small danger" @click="confirmRemoveItem(item.resourceId, item.title)"><Trash2 :size="13" />删除</button>
              </div></td>
            </tr>
          </tbody>
        </table>
      </div>

      <div v-if="activeTab !== 'categories'" class="pagination">
        <span>共 {{ total }} 条 · 第 {{ page }} 页</span>
        <button class="button small" :disabled="page <= 1 || loading" @click="turnPage(page - 1)">上一页</button>
        <button class="button small" :disabled="page * pageSize >= total || loading" @click="turnPage(page + 1)">下一页</button>
      </div>
    </section>

    <div v-if="showEditor" class="modal-backdrop" @click.self="showEditor = false">
      <form class="modal content-modal" @submit.prevent="save">
        <div class="modal-header">
          <h2>{{ editingId == null ? '新增' : '编辑' }}{{ tabTitle }}</h2>
          <button class="icon-button" type="button" @click="showEditor = false"><X :size="18" /></button>
        </div>
        <div class="modal-body form-grid">
          <template v-if="activeTab === 'categories'">
            <div class="field"><label>名称</label><input v-model="form.name" class="input" maxlength="50" required /></div>
            <div class="field"><label>父分类</label><select v-model="form.parentId" class="select"><option value="">无</option><option v-for="item in categories.filter((item) => item.categoryId !== editingId)" :key="item.categoryId" :value="item.categoryId">{{ item.categoryName }}</option></select></div>
            <div class="field full"><label>描述</label><textarea v-model="form.description" class="textarea" rows="3"></textarea></div>
            <div class="field"><label>排序</label><input v-model.number="form.sortOrder" class="input" type="number" /></div>
            <label class="check-field"><input v-model="form.isEnabled" type="checkbox" />启用分类</label>
          </template>
          <template v-else>
            <div class="field"><label>分类</label><select v-model="form.categoryId" class="select" required><option value="" disabled>请选择</option><option v-for="item in categories" :key="item.categoryId" :value="item.categoryId">{{ item.categoryName }}</option></select></div>
            <label class="check-field"><input v-model="form.isPublished" type="checkbox" />发布后对用户可见</label>
            <template v-if="activeTab === 'articles'">
              <div class="field full"><label>标题</label><input v-model="form.title" class="input" maxlength="200" required /></div>
              <div class="field"><label>作者</label><input v-model="form.author" class="input" maxlength="100" /></div>
              <div class="field"><label>来源</label><input v-model="form.source" class="input" maxlength="200" /></div>
              <label class="check-field"><input v-model="form.isFeatured" type="checkbox" />设为精选文章</label>
              <div class="field full"><label>摘要</label><textarea v-model="form.summary" class="textarea" rows="2"></textarea></div>
              <div class="field full"><label>正文</label><textarea v-model="form.content" class="textarea code-area" rows="12" required></textarea></div>
            </template>
            <template v-else-if="activeTab === 'qna'">
              <div class="field full"><label>问题</label><textarea v-model="form.question" class="textarea" rows="3" required></textarea></div>
              <div class="field"><label>专家姓名</label><input v-model="form.expertName" class="input" maxlength="100" /></div>
              <div class="field"><label>专家头衔</label><input v-model="form.expertTitle" class="input" maxlength="200" /></div>
              <label class="check-field"><input v-model="form.isVerified" type="checkbox" />已完成专业验证</label>
              <div class="field full"><label>答案</label><textarea v-model="form.answer" class="textarea code-area" rows="10" required></textarea></div>
            </template>
            <template v-else>
              <div class="field full"><label>标题</label><input v-model="form.title" class="input" maxlength="200" required /></div>
              <div class="field"><label>类型</label><select v-model="form.resourceType" class="select"><option>LINK</option><option>VIDEO</option><option>AUDIO</option><option>PDF</option><option>TOOL</option></select></div>
              <div class="field"><label>外部地址</label><input v-model="form.externalUrl" class="input" type="url" /></div>
              <div class="field full"><label>描述</label><textarea v-model="form.description" class="textarea" rows="4"></textarea></div>
            </template>
            <div class="field full"><label>标签（逗号分隔）</label><input v-model="form.tags" class="input" placeholder="心理健康, 睡眠" /></div>
          </template>
        </div>
        <div class="modal-actions">
          <button class="button" type="button" @click="showEditor = false">取消</button>
          <button class="button primary" :disabled="saving" type="submit">{{ saving ? '保存中...' : '保存' }}</button>
        </div>
      </form>
    </div>
    <ConfirmDialog
      :open="confirmRemove != null"
      title="删除内容"
      :message="confirmRemove ? `确定删除“${confirmRemove.label}”吗？此操作不可撤销。` : ''"
      confirm-label="删除"
      danger
      @confirm="remove"
      @cancel="confirmRemove = null"
    />
  </div>
</template>

<style scoped>
.tabs, .filter-bar { display: flex; gap: 10px; margin-bottom: 16px; }
.tab { border: 1px solid var(--border); background: var(--surface); color: var(--muted); border-radius: 10px; padding: 9px 16px; cursor: pointer; }
.tab.active { background: var(--accent); border-color: var(--accent); color: white; }
.filter-bar { padding: 16px 16px 0; }
.content-modal { width: min(760px, calc(100vw - 32px)); }
.code-area { font-family: inherit; line-height: 1.65; }
.check-field { display: flex; align-items: center; gap: 8px; color: var(--muted); }
@media (max-width: 720px) {
  .tabs, .filter-bar { flex-wrap: wrap; }
}
</style>
