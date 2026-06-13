import {
  ServerRsApiError,
  createMemoryTokenStore,
  createServerRsClient,
} from '../dist/index.js'

const baseUrl = process.env.SERVERRS_BASE_URL || 'http://127.0.0.1:8080'
const username = process.env.SERVERRS_ADMIN_USERNAME
const password = process.env.SERVERRS_ADMIN_PASSWORD

if (!username || !password) {
  throw new Error('SERVERRS_ADMIN_USERNAME and SERVERRS_ADMIN_PASSWORD are required')
}

const client = createServerRsClient({
  baseUrl,
  tokenStore: createMemoryTokenStore(),
  timeoutMs: 60_000,
})

let categoryId
let childCategoryId
let articleId
let qnaId
let resourceId

function assert(condition, message) {
  if (!condition) throw new Error(message)
}

async function cleanup() {
  if (articleId) await client.admin.deletePsychologyArticle(articleId).catch(() => {})
  if (qnaId) await client.admin.deletePsychologyQna(qnaId).catch(() => {})
  if (resourceId) await client.admin.deletePsychologyResource(resourceId).catch(() => {})
  if (childCategoryId) await client.admin.deletePsychologyCategory(childCategoryId).catch(() => {})
  if (categoryId) await client.admin.deletePsychologyCategory(categoryId).catch(() => {})
}

try {
  const login = await client.auth.login({ username, password, device_id: 'sdk-admin-smoke' })
  assert(['ADMIN', 'SUPER_ADMIN'].includes(login.user.role), 'test user is not an administrator')

  const category = await client.admin.createPsychologyCategory({
    name: `SDK 管理测试 ${Date.now()}`,
    description: '管理员 SDK 真实链路测试',
    sortOrder: 999,
  })
  categoryId = category.categoryId
  const childCategory = await client.admin.createPsychologyCategory({
    parentId: categoryId,
    name: `SDK 子分类 ${Date.now()}`,
  })
  childCategoryId = childCategory.categoryId
  try {
    await client.admin.updatePsychologyCategory(categoryId, {
      parentId: childCategoryId,
      name: category.categoryName,
    })
    throw new Error('category cycle was unexpectedly accepted')
  } catch (error) {
    assert(
      error instanceof ServerRsApiError && error.status === 400,
      'category cycle did not return validation error',
    )
  }

  const article = await client.admin.createPsychologyArticle({
    categoryId,
    title: 'SDK 心理文章草稿',
    summary: '包含引号与反斜杠的写入测试',
    content: `内容包含单引号 '、反斜杠 \\ 和中文。`,
    author: 'SDK 审核员',
    source: '真实链路测试',
    tags: ['SDK', '真实测试'],
    isFeatured: true,
    isPublished: false,
  })
  articleId = article.articleId
  assert(article.isPublished === false, 'article draft status was not persisted')
  assert(article.isFeatured && article.author === 'SDK 审核员', 'article review metadata mismatch')

  const articlePage = await client.admin.psychologyArticles({
    categoryId,
    isPublished: false,
  })
  assert(articlePage.items.some((item) => item.articleId === articleId), 'draft article missing')

  const updatedArticle = await client.admin.updatePsychologyArticle(articleId, {
    categoryId,
    title: 'SDK 心理文章已发布',
    content: article.content,
    author: article.author ?? undefined,
    source: article.source ?? undefined,
    tags: ['SDK'],
    isFeatured: true,
    isPublished: true,
  })
  assert(updatedArticle.isPublished, 'article publish update failed')

  const qna = await client.admin.createPsychologyQna({
    categoryId,
    question: 'SDK 管理链路是否正常？',
    answer: '正常。',
    expertName: 'SDK 审核员',
    expertTitle: '自动化测试',
    tags: ['SDK'],
    isVerified: true,
    isPublished: false,
  })
  qnaId = qna.qnaId
  assert(!qna.isPublished, 'qna draft status was not persisted')
  assert(qna.isVerified && qna.expertTitle === '自动化测试', 'qna verification metadata mismatch')

  const resource = await client.admin.createPsychologyResource({
    categoryId,
    title: 'SDK 外部资源',
    resourceType: 'LINK',
    externalUrl: 'https://example.com/',
    tags: ['SDK'],
    isPublished: true,
  })
  resourceId = resource.resourceId
  assert(resource.externalUrl === 'https://example.com/', 'resource URL mismatch')

  const categories = await client.admin.psychologyCategories()
  assert(categories.some((item) => item.categoryId === categoryId), 'created category missing')

  console.log('Admin psychology SDK real API smoke passed.')
} finally {
  await cleanup()
  if (process.env.SERVERRS_ADMIN_DELETE_SELF === 'true') {
    await client.users.deleteMe().catch(() => {})
  }
}
