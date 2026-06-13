import {
  ServerRsApiError,
  createMemoryTokenStore,
  createServerRsClient,
} from '../dist/index.js'

const baseUrl = process.env.SERVERRS_BASE_URL || 'http://127.0.0.1:8080'
const tokenStore = createMemoryTokenStore()
const client = createServerRsClient({ baseUrl, tokenStore, timeoutMs: 180_000 })
const suffix = Date.now().toString(36)
const username = `sdk_${suffix}`.slice(0, 32)
const password = `SdkSmoke_${suffix}!`

let postId
let commentId
let diaryId
let assessmentId
let objectId
let secondPostId
let secondClient

function assert(condition, message) {
  if (!condition) throw new Error(message)
}

async function step(name, action) {
  process.stdout.write(`- ${name} ... `)
  const result = await action()
  console.log('PASS')
  return result
}

async function cleanup() {
  if (commentId && postId) {
    await client.community.deleteComment(postId, commentId).catch(() => {})
  }
  if (postId) await client.community.deletePost(postId).catch(() => {})
  if (secondPostId) await client.community.deletePost(secondPostId).catch(() => {})
  if (diaryId) await client.diaries.delete(diaryId).catch(() => {})
  if (assessmentId) await client.depression.deleteAssessment(assessmentId).catch(() => {})
  if (objectId) await client.objects.delete(objectId).catch(() => {})
  if (secondClient) await secondClient.users.deleteMe().catch(() => {})
  await client.users.deleteMe().catch(() => {})
}

try {
  await step('health', async () => {
    const health = await client.health()
    assert(health.status === 'up', 'health status is not up')
  })

  const login = await step('register and token storage', async () => {
    const result = await client.auth.register({ username, password, device_id: 'sdk-smoke' })
    assert(result.user.username === username, 'registered username mismatch')
    assert(await tokenStore.getAccessToken(), 'access token was not stored')
    return result
  })

  await step('user and profile', async () => {
    const nickname = `SDK ${suffix}`
    const me = await client.users.updateMe({ nickname })
    assert(me.nickname === nickname, 'nickname update failed')
    await client.users.updateProfile({ nickname, interests: ['SDK smoke'] })
    const profile = await client.users.profile()
    assert(profile.nickname === nickname, 'profile nickname was not persisted')
  })

  await step('session and LLM', async () => {
    const session = await client.sessions.create({})
    const reply = await client.sessions.sendMessage(session.session_id, {
      text: '请只回复“SDK链路正常”。',
    })
    assert(Boolean(reply.reply), 'LLM reply is empty')
  })

  await step('diary CRUD', async () => {
    const diary = await client.diaries.create({
      title: 'SDK smoke',
      content: '今天完成了一次真实 SDK 链路测试。',
    })
    diaryId = diary.id
    const updated = await client.diaries.update(diary.id, {
      content: '今天完成了一次真实 SDK 链路测试，结果良好。',
    })
    assert(updated.content.includes('结果良好'), 'diary update failed')
    const diaries = await client.diaries.list({ page: 1, pageSize: 10 })
    assert(diaries.some((item) => item.id === diary.id), 'diary list omitted created item')
  })

  await step('community CRUD and likes', async () => {
    const post = await client.community.createPost({
      title: 'SDK smoke',
      content: 'SDK community integration test',
    })
    postId = post.post_id
    const comment = await client.community.createComment(post.post_id, {
      content: 'SDK comment',
    })
    commentId = comment.comment_id
    await client.community.likePost(post.post_id)
    await client.community.unlikePost(post.post_id)
    await client.community.likeComment(post.post_id, comment.comment_id)
    await client.community.unlikeComment(post.post_id, comment.comment_id)

    const secondPost = await client.community.createPost({
      title: 'SDK parent validation',
      content: 'A different post',
    })
    secondPostId = secondPost.post_id
    try {
      await client.community.createComment(secondPost.post_id, {
        content: 'invalid cross-post reply',
        parent_comment_id: comment.comment_id,
      })
      throw new Error('cross-post parent comment was accepted')
    } catch (error) {
      assert(error instanceof ServerRsApiError && error.status === 400, 'cross-post parent validation failed')
    }
  })

  await step('object upload ownership chain', async () => {
    const payload = new Blob(['serverrs sdk smoke'], { type: 'text/plain' })
    const object = await client.objects.upload(payload, 'document', 'sdk-smoke.txt')
    objectId = object.id
    const metadata = await client.objects.metadata(object.id)
    assert(metadata.sizeBytes === payload.size, 'object metadata size mismatch')
    const downloaded = await client.objects.download(object.id)
    assert((await downloaded.text()) === 'serverrs sdk smoke', 'object content mismatch')

    const secondStore = createMemoryTokenStore()
    secondClient = createServerRsClient({ baseUrl, tokenStore: secondStore })
    await secondClient.auth.register({
      username: `sdk2_${suffix}`.slice(0, 32),
      password,
      device_id: 'sdk-smoke-ownership',
    })
    try {
      await secondClient.objects.metadata(object.id)
      throw new Error('another user unexpectedly accessed object metadata')
    } catch (error) {
      assert(error instanceof ServerRsApiError && error.status === 403, 'object ownership did not return 403')
    }
  })

  await step('depression assessment', async () => {
    const scales = await client.depression.scales()
    if (scales.length === 0) return
    const scale = scales[0]
    const assessment = await client.depression.createAssessment({
      scaleId: scale.scaleId,
      answers: [scale.minScore],
      notes: 'SDK smoke',
    })
    assessmentId = assessment.assessmentId
    assert(assessment.severityLevel.length > 0, 'assessment severity is empty')
    const loaded = await client.depression.assessment(assessment.assessmentId)
    assert(loaded.severityLevel.length > 0, 'stored assessment severity is empty')
  })

  await step('public knowledge and music contracts', async () => {
    const [articles, qna, resources, music] = await Promise.all([
      client.psychology.articles({ page: 1, pageSize: 2 }),
      client.psychology.qna({ page: 1, pageSize: 2 }),
      client.psychology.resources({ page: 1, pageSize: 2 }),
      client.music.tracks({ page: 1, pageSize: 2 }),
    ])
    assert(Array.isArray(articles.items), 'article page contract mismatch')
    assert(Array.isArray(qna.items), 'qna page contract mismatch')
    assert(Array.isArray(resources.items), 'resource page contract mismatch')
    assert(Array.isArray(music.items), 'music page contract mismatch')
  })

  await step('admin authorization boundary', async () => {
    try {
      await client.admin.users({ page: 1, pageSize: 1 })
      throw new Error('normal user unexpectedly accessed admin API')
    } catch (error) {
      assert(error instanceof ServerRsApiError && error.status === 403, 'admin API did not return 403')
    }
  })

  await step('refresh token rotation', async () => {
    const before = await tokenStore.getRefreshToken()
    const refreshed = await client.auth.refresh()
    assert(refreshed.refreshToken !== before, 'refresh token was not rotated')
    assert((await client.auth.me()).id === login.user.id, 'refreshed access token is invalid')
  })

  await cleanup()
  console.log('SDK real API smoke passed.')
} catch (error) {
  await cleanup()
  console.error(error)
  process.exitCode = 1
}
