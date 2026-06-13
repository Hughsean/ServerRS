<template>
  <main class="not-found">
    <div>
      <span>404</span>
      <h1>页面不存在</h1>
      <p>这个地址不属于当前管理控制台。</p>
      <RouterLink class="button primary" to="/">返回控制台</RouterLink>
    </div>
  </main>
</template>

<style scoped>
.not-found {
  display: grid;
  min-height: 100vh;
  place-items: center;
  padding: 30px;
  text-align: center;
}

.not-found > div {
  display: grid;
  justify-items: center;
  gap: 12px;
}

.not-found span {
  color: var(--brand);
  font-family: Georgia, serif;
  font-size: 72px;
}

.not-found h1 {
  font-family: Georgia, "Noto Serif SC", serif;
  font-size: 30px;
}

.not-found p {
  margin-bottom: 10px;
  color: var(--muted);
}
</style>
