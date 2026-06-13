import { createRouter, createWebHistory } from 'vue-router'

const router = createRouter({
  history: createWebHistory(import.meta.env.BASE_URL),
  routes: [
    {
      path: '/login',
      name: 'login',
      component: () => import('@/views/LoginView.vue'),
      meta: { public: true },
    },
    {
      path: '/',
      component: () => import('@/layouts/AdminLayout.vue'),
      children: [
        { path: '', name: 'dashboard', component: () => import('@/views/DashboardView.vue') },
        { path: 'users', name: 'users', component: () => import('@/views/UsersView.vue') },
        { path: 'risks', name: 'risks', component: () => import('@/views/RiskView.vue') },
        {
          path: 'risks/:id',
          name: 'risk-detail',
          component: () => import('@/views/RiskDetailView.vue'),
        },
        {
          path: 'knowledge',
          name: 'knowledge',
          component: () => import('@/views/KnowledgeReviewsView.vue'),
        },
        {
          path: 'knowledge/:id',
          name: 'knowledge-detail',
          component: () => import('@/views/KnowledgeReviewDetailView.vue'),
        },
        { path: 'music', name: 'music', component: () => import('@/views/MusicView.vue') },
      ],
    },
    {
      path: '/:pathMatch(.*)*',
      name: 'not-found',
      component: () => import('@/views/NotFoundView.vue'),
      meta: { public: true },
    },
  ],
})

router.beforeEach(async (to) => {
  const { useAuthStore } = await import('@/stores/auth')
  const auth = useAuthStore()
  if (!auth.initialized) await auth.restore()

  if (to.meta.public) {
    if (to.name === 'login' && auth.isAdmin) return { name: 'dashboard' }
    return true
  }
  if (!auth.isAdmin) return { name: 'login', query: { redirect: to.fullPath } }
  return true
})

export default router
