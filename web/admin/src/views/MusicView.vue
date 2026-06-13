<script setup lang="ts">
import type { CreateMusicTrackRequest, MusicTrack } from '@serverrs/sdk'
import { Music2, Pencil, Plus, Trash2, Upload, X } from '@lucide/vue'
import { onMounted, reactive, ref, shallowRef } from 'vue'

import { api } from '@/lib/sdk'
import { errorMessage } from '@/utils/format'

const tracks = shallowRef<MusicTrack[]>([])
const loading = ref(false)
const saving = ref(false)
const error = ref('')
const search = ref('')
const category = ref('')
const statusFilter = ref<'' | '0' | '1'>('')
const page = ref(1)
const pageSize = 20
const total = ref(0)
const showCreate = ref(false)
const editing = shallowRef<MusicTrack | null>(null)
const audioFile = ref<File | null>(null)

const createForm = reactive({
  title: '',
  artist: '',
  album: '',
  category: '',
  description: '',
  lyrics: '',
})

const editForm = reactive({
  title: '',
  artist: '',
  album: '',
  category: '',
  description: '',
  lyrics: '',
})

async function load(reset = false) {
  if (reset) page.value = 1
  loading.value = true
  error.value = ''
  try {
    const result = await api.admin.tracks({
      page: page.value,
      pageSize,
      search: search.value || undefined,
      category: category.value || undefined,
      status: statusFilter.value === '' ? undefined : Number(statusFilter.value) as 0 | 1,
    })
    tracks.value = result.items
    total.value = result.total
  } catch (cause) {
    error.value = errorMessage(cause)
  } finally {
    loading.value = false
  }
}

function handleFile(event: Event) {
  audioFile.value = (event.target as HTMLInputElement).files?.[0] ?? null
}

async function createTrack() {
  if (!audioFile.value) {
    error.value = '请选择音频文件'
    return
  }
  saving.value = true
  error.value = ''
  try {
    const payload: CreateMusicTrackRequest = {
      title: createForm.title,
      artist: createForm.artist || undefined,
      album: createForm.album || undefined,
      category: createForm.category || undefined,
      description: createForm.description || undefined,
      lyrics: createForm.lyrics || undefined,
      fileData: await fileToBase64(audioFile.value),
      mimeType: audioFile.value.type || 'application/octet-stream',
    }
    await api.admin.createTrack(payload)
    Object.assign(createForm, {
      title: '',
      artist: '',
      album: '',
      category: '',
      description: '',
      lyrics: '',
    })
    audioFile.value = null
    showCreate.value = false
    await load(true)
  } catch (cause) {
    error.value = errorMessage(cause)
  } finally {
    saving.value = false
  }
}

function startEdit(trackId: number) {
  const track = tracks.value.find((item) => item.musicId === trackId)
  if (!track) return
  editing.value = track
  Object.assign(editForm, {
    title: track.title,
    artist: track.artist ?? '',
    album: track.album ?? '',
    category: track.category ?? '',
    description: track.description ?? '',
    lyrics: track.lyrics ?? '',
  })
}

async function saveEdit() {
  if (!editing.value) return
  saving.value = true
  error.value = ''
  try {
    const updated = await api.admin.updateTrack(editing.value.musicId, {
      title: editForm.title,
      artist: editForm.artist || null,
      album: editForm.album || null,
      category: editForm.category || null,
      description: editForm.description || null,
      lyrics: editForm.lyrics || null,
    })
    Object.assign(editing.value, updated)
    editing.value = null
  } catch (cause) {
    error.value = errorMessage(cause)
  } finally {
    saving.value = false
  }
}

async function removeTrack(trackId: number, title: string) {
  if (!window.confirm(`确定删除音乐“${title}”吗？`)) return
  saving.value = true
  try {
    await api.admin.deleteTrack(trackId)
    await load()
  } catch (cause) {
    error.value = errorMessage(cause)
  } finally {
    saving.value = false
  }
}

async function toggleTrackStatus(track: MusicTrack) {
  saving.value = true
  error.value = ''
  try {
    const updated = await api.admin.updateTrack(track.musicId, {
      status: track.status === 1 ? 0 : 1,
    })
    Object.assign(track, updated)
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

function fileToBase64(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader()
    reader.onerror = () => reject(reader.error ?? new Error('读取文件失败'))
    reader.onload = () => {
      const value = String(reader.result)
      resolve(value.includes(',') ? value.slice(value.indexOf(',') + 1) : value)
    }
    reader.readAsDataURL(file)
  })
}

onMounted(() => load())
</script>

<template>
  <div>
    <header class="page-heading">
      <div>
        <h1>音乐资源</h1>
        <p>管理音乐元数据与音频文件，供普通用户端检索和播放。</p>
      </div>
      <div class="toolbar">
        <input v-model="search" class="input search-input" placeholder="标题或作者" @keyup.enter="load(true)" />
        <input v-model="category" class="input category-input" placeholder="分类" @keyup.enter="load(true)" />
        <select v-model="statusFilter" class="select" @change="load(true)">
          <option value="">全部状态</option>
          <option value="1">已启用</option>
          <option value="0">已停用</option>
        </select>
        <button class="button" @click="load(true)">搜索</button>
        <button class="button primary" @click="showCreate = true"><Plus :size="16" />新增音乐</button>
      </div>
    </header>

    <p v-if="error" class="notice">{{ error }}</p>

    <section class="card">
      <div v-if="loading" class="loading-state">正在加载音乐资源...</div>
      <div v-else-if="!tracks.length" class="empty-state">
        <div><Music2 :size="34" /><p>暂无音乐资源</p></div>
      </div>
      <div v-else class="table-wrap">
        <table class="data-table">
          <thead>
            <tr><th>音乐</th><th>专辑</th><th>分类</th><th>状态</th><th>时长</th><th>格式</th><th>大小</th><th>操作</th></tr>
          </thead>
          <tbody>
            <tr v-for="track in tracks" :key="track.musicId">
              <td>
                <div class="cell-title">
                  <strong>{{ track.title }}</strong>
                  <span>{{ track.artist || '未知艺术家' }} · #{{ track.musicId }}</span>
                </div>
              </td>
              <td>{{ track.album || '-' }}</td>
              <td><span class="badge">{{ track.category || '未分类' }}</span></td>
              <td><span class="badge">{{ track.status === 1 ? '已启用' : '已停用' }}</span></td>
              <td>{{ track.duration == null ? '-' : `${Math.floor(track.duration / 60)}:${String(track.duration % 60).padStart(2, '0')}` }}</td>
              <td>{{ track.mimeType }}</td>
              <td>{{ (track.fileSize / 1024 / 1024).toFixed(2) }} MB</td>
              <td>
                <div class="row-actions">
                  <button class="button small" :disabled="saving" @click="toggleTrackStatus(track)">
                    {{ track.status === 1 ? '停用' : '启用' }}
                  </button>
                  <button class="button small" @click="startEdit(track.musicId)"><Pencil :size="13" />编辑</button>
                  <button class="button small danger" :disabled="saving" @click="removeTrack(track.musicId, track.title)"><Trash2 :size="13" />删除</button>
                </div>
              </td>
            </tr>
          </tbody>
        </table>
      </div>
      <div class="pagination">
        <span>共 {{ total }} 首 · 第 {{ page }} 页</span>
        <button class="button small" :disabled="page <= 1 || loading" @click="turnPage(page - 1)">上一页</button>
        <button class="button small" :disabled="page * pageSize >= total || loading" @click="turnPage(page + 1)">下一页</button>
      </div>
    </section>

    <div v-if="showCreate" class="modal-backdrop" @click.self="showCreate = false">
      <form class="modal" @submit.prevent="createTrack">
        <div class="modal-header">
          <h2>新增音乐</h2>
          <button class="icon-button" type="button" @click="showCreate = false"><X :size="18" /></button>
        </div>
        <div class="modal-body form-grid">
          <div class="field full"><label>音频文件</label><input accept="audio/*" required type="file" @change="handleFile" /></div>
          <div class="field"><label>标题</label><input v-model="createForm.title" class="input" required /></div>
          <div class="field"><label>艺术家</label><input v-model="createForm.artist" class="input" /></div>
          <div class="field"><label>专辑</label><input v-model="createForm.album" class="input" /></div>
          <div class="field"><label>分类</label><input v-model="createForm.category" class="input" /></div>
          <div class="field full"><label>描述</label><textarea v-model="createForm.description" class="textarea"></textarea></div>
          <div class="field full"><label>歌词</label><textarea v-model="createForm.lyrics" class="textarea"></textarea></div>
        </div>
        <div class="modal-footer">
          <button class="button" type="button" @click="showCreate = false">取消</button>
          <button class="button primary" :disabled="saving" type="submit"><Upload :size="15" />{{ saving ? '上传中...' : '上传并创建' }}</button>
        </div>
      </form>
    </div>

    <div v-if="editing" class="modal-backdrop" @click.self="editing = null">
      <form class="modal" @submit.prevent="saveEdit">
        <div class="modal-header">
          <h2>编辑音乐</h2>
          <button class="icon-button" type="button" @click="editing = null"><X :size="18" /></button>
        </div>
        <div class="modal-body form-grid">
          <div class="field"><label>标题</label><input v-model="editForm.title" class="input" required /></div>
          <div class="field"><label>艺术家</label><input v-model="editForm.artist" class="input" /></div>
          <div class="field"><label>专辑</label><input v-model="editForm.album" class="input" /></div>
          <div class="field"><label>分类</label><input v-model="editForm.category" class="input" /></div>
          <div class="field full"><label>描述</label><textarea v-model="editForm.description" class="textarea"></textarea></div>
          <div class="field full"><label>歌词</label><textarea v-model="editForm.lyrics" class="textarea"></textarea></div>
        </div>
        <div class="modal-footer">
          <button class="button" type="button" @click="editing = null">取消</button>
          <button class="button primary" :disabled="saving" type="submit">{{ saving ? '保存中...' : '保存修改' }}</button>
        </div>
      </form>
    </div>
  </div>
</template>

<style scoped>
.notice {
  margin-bottom: 16px;
}

.category-input {
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

.form-grid {
  grid-template-columns: 1fr 1fr;
}

.form-grid .full {
  grid-column: 1 / -1;
}

input[type="file"] {
  padding: 11px;
  border: 1px dashed #aebeb8;
  border-radius: 10px;
  background: #f8faf9;
}

@media (max-width: 620px) {
  .form-grid {
    grid-template-columns: 1fr;
  }

  .form-grid .full {
    grid-column: auto;
  }
}
</style>
