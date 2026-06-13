<script setup lang="ts">
import {
  BookOpen,
  BookOpenCheck,
  ChevronLeft,
  ChevronRight,
  CircleUserRound,
  Gauge,
  LogOut,
  Menu,
  Music2,
  ShieldAlert,
  Users,
  X,
} from '@lucide/vue'
import { ref } from 'vue'
import { useRouter } from 'vue-router'

import { useAuthStore } from '@/stores/auth'

const auth = useAuthStore()
const router = useRouter()
const collapsed = ref(false)
const mobileOpen = ref(false)

const navigation = [
  { to: '/', label: '运行概览', icon: Gauge },
  { to: '/users', label: '用户管理', icon: Users },
  { to: '/risks', label: '风险会话', icon: ShieldAlert },
  { to: '/knowledge', label: '知识审核', icon: BookOpenCheck },
  { to: '/psychology', label: '心理内容', icon: BookOpen },
  { to: '/music', label: '音乐资源', icon: Music2 },
]

async function signOut() {
  await auth.logout()
  await router.replace('/login')
}
</script>

<template>
  <div class="admin-shell" :class="{ 'is-collapsed': collapsed }">
    <aside class="sidebar" :class="{ 'is-open': mobileOpen }">
      <div class="brand">
        <div class="brand-mark">S</div>
        <div class="brand-copy">
          <strong>ServerRS</strong>
          <span>管理控制台</span>
        </div>
        <button class="icon-button mobile-only" aria-label="关闭菜单" @click="mobileOpen = false">
          <X :size="19" />
        </button>
      </div>

      <nav class="sidebar-nav">
        <RouterLink
          v-for="item in navigation"
          :key="item.to"
          :to="item.to"
          class="nav-item"
          @click="mobileOpen = false"
        >
          <component :is="item.icon" :size="19" />
          <span>{{ item.label }}</span>
        </RouterLink>
      </nav>

      <div class="sidebar-footer">
        <div class="server-indicator">
          <span class="status-dot"></span>
          <span>服务接口</span>
          <strong>已连接</strong>
        </div>
        <button class="collapse-button" @click="collapsed = !collapsed">
          <ChevronRight v-if="collapsed" :size="18" />
          <ChevronLeft v-else :size="18" />
          <span>收起导航</span>
        </button>
      </div>
    </aside>

    <div v-if="mobileOpen" class="sidebar-backdrop" @click="mobileOpen = false"></div>

    <section class="workspace">
      <header class="topbar">
        <button class="icon-button mobile-only" aria-label="打开菜单" @click="mobileOpen = true">
          <Menu :size="21" />
        </button>
        <div class="topbar-context">
          <span class="eyebrow">ADMIN CONSOLE</span>
          <strong>系统管理中心</strong>
        </div>
        <div class="account">
          <CircleUserRound :size="22" />
          <div>
            <strong>{{ auth.user?.username }}</strong>
            <span>{{ auth.user?.role }}</span>
          </div>
          <button class="icon-button" title="退出登录" @click="signOut">
            <LogOut :size="18" />
          </button>
        </div>
      </header>

      <main class="page-container">
        <RouterView />
      </main>
    </section>
  </div>
</template>
