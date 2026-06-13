<script setup lang="ts">
import { ArrowRight, KeyRound, LockKeyhole, ShieldCheck, UserRound } from '@lucide/vue'
import { ref } from 'vue'
import { useRoute, useRouter } from 'vue-router'

import { useAuthStore } from '@/stores/auth'
import { errorMessage } from '@/utils/format'

const auth = useAuthStore()
const route = useRoute()
const router = useRouter()
const username = ref('')
const password = ref('')
const error = ref('')

async function submit() {
  error.value = ''
  try {
    await auth.login(username.value.trim(), password.value)
    const redirect = typeof route.query.redirect === 'string' ? route.query.redirect : '/'
    await router.replace(redirect)
  } catch (cause) {
    error.value = errorMessage(cause)
  }
}
</script>

<template>
  <main class="login-page">
    <section class="login-intro">
      <div class="login-brand">
        <div class="brand-mark">S</div>
        <div>
          <strong>ServerRS</strong>
          <span>ADMINISTRATION</span>
        </div>
      </div>

      <div class="intro-copy">
        <span class="eyebrow">KNOWLEDGE · SAFETY · OPERATIONS</span>
        <h1>把系统的复杂度，<br />留在可控的地方。</h1>
        <p>统一管理用户、风险会话、知识审核与媒体资源。所有操作直接连接当前 ServerRS 接口。</p>
      </div>

      <div class="intro-points">
        <div><ShieldCheck :size="19" /><span>管理员角色校验</span></div>
        <div><KeyRound :size="19" /><span>令牌本地安全存储</span></div>
      </div>
    </section>

    <section class="login-panel">
      <form class="login-card" @submit.prevent="submit">
        <div class="login-card-heading">
          <span>欢迎回来</span>
          <h2>登录管理控制台</h2>
          <p>请使用具有 ADMIN 或 SUPER_ADMIN 角色的账号。</p>
        </div>

        <div class="field">
          <label for="username">用户名</label>
          <div class="input-shell">
            <UserRound :size="18" />
            <input
              id="username"
              v-model="username"
              autocomplete="username"
              placeholder="输入管理员用户名"
              required
            />
          </div>
        </div>

        <div class="field">
          <label for="password">密码</label>
          <div class="input-shell">
            <LockKeyhole :size="18" />
            <input
              id="password"
              v-model="password"
              autocomplete="current-password"
              minlength="8"
              placeholder="至少 8 位"
              required
              type="password"
            />
          </div>
        </div>

        <p v-if="error" class="login-error">{{ error }}</p>

        <button class="login-submit" :disabled="auth.loading" type="submit">
          <span>{{ auth.loading ? '正在验证...' : '进入控制台' }}</span>
          <ArrowRight :size="18" />
        </button>
      </form>
    </section>
  </main>
</template>

<style scoped>
.login-page {
  display: grid;
  min-height: 100vh;
  grid-template-columns: minmax(420px, 1.05fr) minmax(420px, 0.95fr);
}

.login-intro {
  position: relative;
  display: flex;
  overflow: hidden;
  flex-direction: column;
  justify-content: space-between;
  padding: clamp(34px, 5vw, 72px);
  color: #edf7f3;
  background:
    linear-gradient(145deg, rgba(10, 46, 40, 0.96), rgba(21, 82, 70, 0.92)),
    #123c35;
}

.login-intro::after {
  position: absolute;
  right: -160px;
  bottom: -220px;
  width: 560px;
  height: 560px;
  border: 1px solid rgba(255, 255, 255, 0.12);
  border-radius: 50%;
  box-shadow:
    0 0 0 70px rgba(255, 255, 255, 0.025),
    0 0 0 140px rgba(255, 255, 255, 0.02);
  content: "";
}

.login-brand {
  display: flex;
  align-items: center;
  gap: 13px;
}

.login-brand > div:last-child {
  display: flex;
  flex-direction: column;
}

.login-brand strong {
  font-size: 18px;
}

.login-brand span {
  color: rgba(237, 247, 243, 0.58);
  font-size: 9px;
  letter-spacing: 0.18em;
}

.intro-copy {
  position: relative;
  z-index: 1;
  max-width: 610px;
}

.intro-copy .eyebrow {
  color: #e0ad68;
}

.intro-copy h1 {
  margin-top: 18px;
  font-family: Georgia, "Noto Serif SC", serif;
  font-size: clamp(38px, 5vw, 64px);
  font-weight: 500;
  line-height: 1.14;
  letter-spacing: -0.045em;
}

.intro-copy p {
  max-width: 560px;
  margin-top: 22px;
  color: rgba(237, 247, 243, 0.66);
  font-size: 15px;
  line-height: 1.85;
}

.intro-points {
  position: relative;
  z-index: 1;
  display: flex;
  gap: 28px;
}

.intro-points div {
  display: flex;
  align-items: center;
  gap: 8px;
  color: rgba(237, 247, 243, 0.78);
  font-size: 12px;
}

.login-panel {
  display: grid;
  place-items: center;
  padding: 30px;
  background:
    radial-gradient(circle at 80% 20%, rgba(216, 145, 58, 0.08), transparent 20rem),
    #f2f5f3;
}

.login-card {
  display: grid;
  width: min(430px, 100%);
  gap: 20px;
  padding: clamp(28px, 5vw, 48px);
  border: 1px solid rgba(205, 216, 212, 0.9);
  border-radius: 22px;
  background: rgba(255, 255, 255, 0.94);
  box-shadow: 0 28px 70px rgba(28, 54, 47, 0.12);
}

.login-card-heading {
  margin-bottom: 8px;
}

.login-card-heading > span {
  color: var(--brand);
  font-size: 12px;
  font-weight: 800;
}

.login-card-heading h2 {
  margin-top: 6px;
  font-family: Georgia, "Noto Serif SC", serif;
  font-size: 30px;
  font-weight: 600;
}

.login-card-heading p {
  margin-top: 8px;
  color: var(--muted);
  font-size: 12px;
}

.input-shell {
  display: flex;
  height: 46px;
  align-items: center;
  gap: 10px;
  padding: 0 13px;
  border: 1px solid #cbd7d2;
  border-radius: 11px;
  color: #77837f;
  background: #fff;
}

.input-shell:focus-within {
  border-color: var(--brand);
  box-shadow: 0 0 0 3px rgba(23, 107, 91, 0.11);
}

.input-shell input {
  min-width: 0;
  flex: 1;
  border: 0;
  outline: 0;
  color: var(--ink);
  background: transparent;
}

.login-error {
  padding: 10px 12px;
  border-radius: 9px;
  color: var(--danger);
  background: var(--danger-soft);
  font-size: 12px;
}

.login-submit {
  display: flex;
  height: 47px;
  align-items: center;
  justify-content: space-between;
  padding: 0 17px;
  border: 0;
  border-radius: 11px;
  color: #fff;
  background: var(--brand);
  font-weight: 750;
  cursor: pointer;
}

.login-submit:hover:not(:disabled) {
  background: var(--brand-dark);
}

.login-submit:disabled {
  cursor: wait;
  opacity: 0.65;
}

@media (max-width: 850px) {
  .login-page {
    grid-template-columns: 1fr;
  }

  .login-intro {
    min-height: 310px;
    padding: 30px;
  }

  .intro-copy h1 {
    font-size: 36px;
  }

  .intro-points {
    display: none;
  }
}
</style>
