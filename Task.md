# ServerRS Web Ingestion 下一轮修复任务书：核心闭环与安全边界修复

## 0. 当前结论

当前 web ingestion 仍不能验收，也不能开启。

上一轮已经完成部分外围修复：

* 默认配置保持 disabled。
* repair SQL 已处理部分 schema 问题。
* 状态机校验已有接入迹象。
* active_page_key 已有 DB 级保护方案。
* Dispatcher 对未知事件不再直接成功。
* CrawlJobCreated handler 有部分实现。

但仍存在核心阻塞问题：

* 已知但未实现的 handler 仍可能返回成功并导致事件被标记为已处理。
* Outbox 原始 SQL 仍存在字符串拼接问题。
* Publish lock 不是有效事务锁。
* UrlDiscovered 处理链路存在 panic、错误 run_id、page upsert 缺失等问题。
* Scheduler 与 CrawlJobCreatedHandler 职责重复。
* Publish / Rollback 没有接入 Qdrant active 操作。
* RetrievalService 没有 web_ingestion active 过滤。
* Embedding provider factory / OpenAI-compatible embedding provider 未完成。
* Chunker 超长 block / 死循环问题未修复。
* SSRF allowed_domains 未生效。

本轮目标不是继续堆文件，而是把这些阻塞问题修到**不会假成功、不会污染 RAG、不会错误发布、不会静默丢事件**。

---

# 1. 总原则

## 1.1 默认仍然禁止开启

修复完成前，必须保持：

* web_ingestion.enabled = false
* scheduler_enabled = false
* dispatcher_enabled = false

本轮结束时，如果仍有核心闭环未完成，也必须继续保持 disabled。

不得为了演示功能而默认开启。

---

## 1.2 不允许“假成功”

以下情况必须失败，不能返回成功：

* handler 未实现。
* handler 只写了 TODO。
* Qdrant 更新失败。
* publish 或 rollback 只改了 DB，没有改 Qdrant。
* Retrieval active 校验不可执行。
* embedding 返回数量不匹配。
* embedding 维度不匹配。
* chunker 无法安全切分。
* source_url、page、run、publish_record 等关键对象缺失。
* 状态机非法流转。
* Outbox mark published / mark failed 未能确认锁归属。

失败时必须：

* outbox 进入 failed 或 dead。
* last_error 记录清楚。
* audit log 记录清楚。
* 不得把事件标记为 published。
* 不得让 staged / failed / old version 被召回。

---

## 1.3 不要求按本任务书写死代码

本任务书不要求使用某个具体 SQL 写法、某个具体 Rust API、某个具体第三方库调用方式。
Claude Code 应按项目当前依赖、SeaORM 版本、Qdrant client 能力和现有代码结构选择合适实现。

但最终行为必须满足本文验收要求。

---

# 2. 本轮修复范围

本轮只修复 P0 / P1 问题，不新增无关功能。

必须修复：

1. Dispatcher 假成功问题。
2. Outbox 幂等与 SQL 安全问题。
3. Scheduler 与 CrawlJobCreatedHandler 职责冲突。
4. UrlDiscovered handler 的 page/run/content_hash 正确性问题。
5. Publish / Rollback 事务边界。
6. Publish / Rollback Qdrant active 操作。
7. RetrievalService web_ingestion active 过滤。
8. Embedding provider factory 与 batch embedding。
9. Chunker 超长 block / 死循环问题。
10. SSRF allowed_domains。
11. 配置 env override 不完整问题。
12. 必要测试补齐。

本轮禁止：

* 修改 AgentRuntime 主聊天流程。
* 修改现有 RAG 入口语义，除非是增加 web_ingestion active 安全过滤。
* 破坏 legacy 知识库召回。
* 使用 in-memory domain/tasks 驱动核心 ingestion。
* 新增真实 API key。
* 新增真实外网测试。
* 跳过失败处理。
* 用注释或 TODO 代替真实行为。

---

# 3. Dispatcher 修复

## 3.1 核心目标

Dispatcher 必须保证：

* 未知事件不能成功。
* 已知但未实现事件不能成功。
* TODO handler 不能返回成功。
* 只有实际完成业务动作后，事件才能被标记为已处理。
* handler 失败必须进入 outbox retry / dead 机制。

## 3.2 必须修改的行为

当前“已知事件匹配到 stub handler，然后 handler 返回成功”的行为必须彻底删除。

所有 handler 只能有两种状态：

1. 完整实现并在完成业务动作后返回成功。
2. 明确返回失败，并写明 handler 未实现或当前条件不满足。

不得存在：

* 空 handler 返回成功。
* TODO handler 返回成功。
* 只打印日志就返回成功。
* 只更新 audit 就返回成功，但没有推进状态机或产生下一事件。

## 3.3 验收标准

必须能证明：

* 未知 event_type 会 failed，不会 published。
* 已知但未实现 event_type 会 failed，不会 published。
* 已实现 handler 成功后才会 mark published。
* handler 失败后 outbox retry_count、next_retry_at、last_error 正确更新。
* retry 耗尽后进入 dead。
* dead 事件不会继续被 claim。

---

# 4. Outbox 修复

## 4.1 核心目标

Outbox 是核心流程驱动，必须满足：

* 事件插入幂等。
* claim 原子。
* processing 锁过期可回收。
* mark published / failed 必须校验锁归属。
* 所有数据库写入必须避免字符串拼接风险。
* 事件重复执行不会产生重复业务数据。

## 4.2 必须修复

### 4.2.1 insert_event 真幂等

重复插入同一个 event_key 时：

* 不报错。
* 不新增重复事件。
* 返回已有事件或明确 no-op。
* 不改变业务语义。

### 4.2.2 SQL 安全

所有 outbox raw SQL 必须改成参数绑定或项目当前数据库抽象支持的等价安全写法。

不得用以下方式作为安全依据：

* “值都是内部常量”。
* “值来自 UUID”。
* “sha256 hex 安全”。
* “手动 replace 单引号”。

payload、last_error、event_type、aggregate_type、locked_by 等都必须按参数处理。

### 4.2.3 Claim 与锁归属

必须保留：

* pending / failed 可 claim。
* processing 且锁过期可 reclaim。
* 每轮 claim 使用唯一 token。
* mark published 必须校验 locked_by。
* mark failed 必须校验 locked_by。
* locked_by 不匹配时不得假装成功。

## 4.3 验收标准

测试必须覆盖：

* 重复 event_key 插入。
* processing 锁过期 reclaim。
* locked_by 不匹配时 mark published 失败。
* locked_by 不匹配时 mark failed 失败。
* last_error 包含引号、URL、JSON 字符时不会破坏 SQL。
* payload 包含引号、URL、JSON 字符时不会破坏 SQL。

---

# 5. Scheduler 与 CrawlJobCreatedHandler 职责修复

## 5.1 核心目标

Scheduler 和 CrawlJobCreatedHandler 的职责必须单一，不能重复发现 URL，不能重复插入 UrlDiscovered，不能跨 source 污染。

## 5.2 推荐职责边界

推荐采用以下语义：

### Scheduler

只负责：

* 扫描 enabled source。
* 为 due source 创建 crawl_job。
* 插入 CrawlJobCreated event。

Scheduler 不负责：

* 直接插入 UrlDiscovered。
* 直接读取 source_urls。
* 直接 fetch。
* 直接创建 run。

### CrawlJobCreatedHandler

只负责：

* 读取 crawl_job。
* 根据 crawl_job.source_id 读取该 source 下 due 且 enabled 的 source_urls。
* 为这些 URL 插入 UrlDiscovered event。
* 更新 crawl_job 状态。
* 不抓网页。
* 不调用 DeepSeek。
* 不调用 embedding。

## 5.3 必须修复的问题

* 删除 Scheduler 和 CrawlJobCreatedHandler 双方重复发 UrlDiscovered 的行为。
* CrawlJobCreatedHandler 必须按 job.source_id 过滤 URL。
* CrawlJobCreatedHandler 不得把其他 source 的 URL 挂到当前 crawl_job 下。
* 重复 UrlDiscovered 必须依赖 event_key 幂等，不得产生重复业务效果。

## 5.4 验收标准

测试必须覆盖：

* Scheduler 只创建 crawl_job 和 CrawlJobCreated。
* CrawlJobCreatedHandler 只处理当前 job.source_id 的 URL。
* 多个 source 同时 due 时不会串 source。
* 同一 crawl_job 重复执行不会插入重复有效事件。

---

# 6. UrlDiscoveredHandler 修复

## 6.1 核心目标

UrlDiscoveredHandler 是网页处理链路的入口，必须做到：

* 不 panic。
* 正确 upsert page。
* 正确计算 content_hash。
* content unchanged 正确 skipped。
* content changed 正确创建 run。
* run 创建后再更新 page.latest_success_run_id。
* run_key 使用当前 pipeline profile，而不是硬编码空字符串。
* 去重基于当前 run_key / version_key，而不是非唯一 content_key。

## 6.2 必须修复

### 6.2.1 禁止 panic

如果 source_url、page、source、crawl_job、payload 字段缺失：

* 返回错误。
* outbox 进入 failed。
* last_error 说明缺失对象。
* 不得 panic。

### 6.2.2 page upsert

UrlDiscoveredHandler 必须确保 web_pages 存在。
如果不存在，必须按 source_id + url_hash 创建。
如果存在，复用现有 page。

不得假设 page 已存在。

### 6.2.3 content unchanged

如果 content_hash 与上一次成功抓取一致，并且当前 pipeline profile 没有变化：

* 不调用 DeepSeek。
* 不调用 embedding。
* 不写 Qdrant。
* 更新 last_fetched_at。
* 插入 IngestionSkipped 或写 audit。
* outbox 正常完成。

不得用 rejected 表示 unchanged。

### 6.2.4 content changed

如果内容变化：

* 先根据当前配置计算 run_key / version_key。
* 以 run_key / version_key 做幂等检查。
* 不得使用非唯一 content_key 作为唯一去重依据。
* 创建 knowledge_ingestion_run。
* 创建成功后再把 page.latest_success_run_id 更新为真实 run_id。
* 插入 PageFetched event。

### 6.2.5 pipeline profile

run_key / version_key 必须基于当前实际配置或当前 profile，包括：

* source_id
* page_id
* content_hash
* llm prompt version
* chunker version
* embedding model
* pipeline version

不得使用硬编码空字符串替代 embedding_model。
不得在不同位置用不同硬编码版本生成 run_key。

## 6.3 验收标准

测试必须覆盖：

* page 不存在时自动创建。
* page 存在时复用。
* content unchanged 不进入 DeepSeek / embedding。
* content changed 创建 run。
* latest_success_run_id 不会写 0。
* run_key 使用当前配置。
* 重复 UrlDiscovered 不创建重复 run。
* 缺失 payload 字段不会 panic。

---

# 7. 状态机与幂等修复

## 7.1 核心目标

状态机必须是业务约束，不只是测试工具。

所有 run 状态推进必须：

* 校验当前状态。
* 校验目标状态。
* 非法流转失败。
* 重复事件到达时允许幂等 no-op。
* 终态不能继续推进。

## 7.2 必须检查

所有 handler 更新 run status/stage 时，都必须经过状态机约束。
不得直接绕过 repository 或直接 update 字段。

## 7.3 验收标准

测试必须覆盖：

* 合法流转。
* 非法流转。
* 同状态重复流转。
* 终态后继续流转失败。
* outbox 重放不会破坏终态。

---

# 8. Publish / Supersede 修复

## 8.1 核心目标

Publish 必须保证：

* 同一 page 同时最多一个 active publish_record。
* 新旧版本不会同时可检索。
* DB 和 Qdrant 的 active 语义一致。
* Qdrant 失败不能被报告为成功。
* 发布流程必须可重试且不会越重试越乱。

## 8.2 事务边界

当前“单独执行锁语句然后继续多个 update”的方式不合格。
必须改为真正事务边界。

要求：

* 发布同一个 page 时必须串行。
* 锁必须在事务内有效。
* 事务内完成 DB 侧 active 切换。
* 事务失败必须整体回滚 DB 侧变更。
* 不得在 autocommit 模式下声称已经持有 publish lock。

具体使用什么 transaction API，由当前项目依赖决定。

## 8.3 Qdrant active 操作

Publish 必须接入 Qdrant 或项目现有 vector adapter。

发布前必须校验：

* 新版本所有 point 存在。
* 新版本所有 point 当前不可检索。
* 旧版本 point 能被下线或删除。

发布过程中必须：

* 下线旧版本 Qdrant points。
* DB 切换旧版本 inactive、新版本 active。
* 激活新版本 Qdrant points。

如果任何 Qdrant 步骤失败：

* 不得声称发布成功。
* 必须记录 audit。
* 必须让 outbox 失败并可重试。
* 必须保证 RetrievalService 不会召回错误版本。

## 8.4 Manifest active

发布成功后必须满足：

* 新 publish_record active。
* 新 chunk_manifest active。
* 新 vector_manifest active。
* 新 Qdrant payload active。
* 旧 publish_record inactive。
* 旧 chunk_manifest inactive。
* 旧 vector_manifest inactive。
* 旧 Qdrant payload inactive 或 point 被删除。

## 8.5 验收标准

测试必须覆盖：

* 首次发布。
* 有旧版本时发布新版本。
* 旧 Qdrant 下线失败时发布失败。
* 新 Qdrant 激活失败时发布失败。
* 同 page 并发发布不会产生两个 active。
* 发布失败后不会出现 DB active 但 Qdrant inactive 且被报告成功。
* 发布成功后只有新版本可召回。

---

# 9. Rollback 修复

## 9.1 核心目标

Rollback 必须能把同一 page 的历史发布版本恢复为 active，并保证 Qdrant 与 DB 一致。

## 9.2 必须满足

Rollback 必须校验：

* current_record 是当前 active。
* target_record 属于同 source_id + page_id。
* target_record 有对应 document、chunks、manifests。
* target Qdrant points 存在；如不存在，必须能恢复或明确失败。
* current Qdrant points 能被下线。

Rollback 成功后：

* current inactive。
* target active。
* current manifests inactive。
* target manifests active。
* current Qdrant inactive 或删除。
* target Qdrant active。
* 插入 KnowledgeRolledBack event。
* 写 audit。

任何失败不得报告成功。

## 9.3 验收标准

测试必须覆盖：

* target 不同 page 时拒绝。
* current 不是 active 时拒绝。
* target 缺 chunks/manifests 时拒绝。
* target Qdrant 缺失时能恢复或失败。
* Qdrant 下线失败时 rollback 失败。
* rollback 成功后只有 target 可召回。

---

# 10. RetrievalService active 过滤修复

## 10.1 核心目标

RetrievalService 必须防止 web ingestion 的 staged、rejected、failed、superseded、rolled_back old version 被召回。

同时不能破坏 legacy RAG 召回。

## 10.2 必须实现

RetrievalService 对每个候选 chunk 必须区分：

### Legacy chunk

保持现有逻辑：

* document status 合法。
* document 未删除。
* visibility 合法。
* chunk status 合法。

### web_page_version chunk

必须额外校验：

* 对应 publish_record active。
* publish_status 是 published。
* chunk_manifest active。
* vector_manifest active。
* document.source_type 与 publish_record 关系一致。
* chunk_id 与 manifest 关系一致。

校验失败则不可返回给 AgentRuntime。

## 10.3 Qdrant 路径

Qdrant 查询可以做兼容过滤，但不能只依赖 Qdrant payload。

必须有 DB 后置校验。
如果 Qdrant filter 不支持表达 legacy 与 web_ingestion 的兼容条件，则允许 overfetch，再做 DB 后置过滤。

## 10.4 Keyword fallback

keyword fallback 必须使用同样 active 校验。
不得出现 vector path 安全、keyword path 泄漏旧版本的情况。

## 10.5 验收标准

测试必须覆盖：

* legacy 文档仍可召回。
* web_page_version staged 不可召回。
* web_page_version superseded 不可召回。
* web_page_version published active 可召回。
* Qdrant path 过滤生效。
* keyword fallback 过滤生效。
* publish_record inactive 时不可召回。
* manifest inactive 时不可召回。

---

# 11. Knowledge 写入闭环

## 11.1 核心目标

Web ingestion 必须真实写入现有知识库，而不是只写 manifest。

必须完成：

* 创建 knowledge_document。
* 创建 knowledge_chunks。
* 批量 embedding。
* 写入现有 embedding / vector index 记录或项目等价结构。
* 写入 Qdrant。
* 写 chunk_manifest。
* 写 vector_manifest。

## 11.2 document 写入语义

每个 publish_record 对应一个独立 knowledge_document。

要求：

* source_type 使用 web_page_version。
* source_id 使用 publish_record_id 或项目确定的唯一版本 ID。
* metadata 保存 web_page_id、source_id、source_url_id、run_id、publish_record_id、url、canonical_url、version_key、content_hash、llm_model、embedding_model、chunker_version。
* 不覆盖旧 document。
* 不复用旧 document 表示新版本。

## 11.3 chunk 写入语义

每个 chunk 必须：

* 归属当前 document。
* 有稳定 chunk_hash。
* 有 chunk_index。
* 有 chunk_type。
* 内容带上下文头。
* manifest 能映射到 publish_record / run / document / chunk。

重复事件重放时不得创建重复 chunk。

## 11.4 Qdrant 写入语义

DocumentIndexed 阶段写入 Qdrant 时：

* point_id 稳定。
* payload 包含 publish_record_id、document_id、chunk_id、source_id、page_id、version_key、content_hash、chunk_hash、embedding_model、ingestion_source。
* active 初始必须为 false。
* 只有 publish 成功后才能变 true。

## 11.5 验收标准

测试必须覆盖：

* 重复 DocumentChunked 不创建重复 document。
* 重复 DocumentChunked 不创建重复 chunks。
* 重复 DocumentIndexed 不创建重复 Qdrant points。
* staged 版本 Qdrant active=false。
* publish 后 Qdrant active=true。

---

# 12. Embedding Provider 修复

## 12.1 核心目标

Embedding 必须与 DeepSeek / distill LLM 完全分离。
配置中的 provider、api_key、batch_size、timeout、dimension 必须实际生效。

## 12.2 必须实现

必须有 embedding provider factory，按配置选择实际 provider。

至少支持：

* 当前已有 embedding provider。
* OpenAI-compatible embedding provider。

OpenAI-compatible embedding provider 必须支持：

* batch input。
* API key。
* timeout。
* 429 / 5xx / timeout retry。
* 返回数量校验。
* 维度校验。
* 错误信息清晰。
* 不使用 DeepSeek distill_llm 配置。
* 不使用 DEEPSEEK_API_KEY，除非用户明确把它配置为 embedding api key，但默认不得混用。

## 12.3 禁止逐 chunk 请求

Web ingestion embedding worker 必须按 batch_size 批量处理。
不得一个 chunk 发一次 embedding 请求。

## 12.4 验收标准

测试必须覆盖：

* provider 配置生效。
* OpenAI-compatible provider batch 请求。
* 返回数量不匹配失败。
* 维度不匹配失败。
* 429 / 5xx retry。
* embedding api_key 与 distill_llm api_key 隔离。
* batch_size 生效。
* 重复 embedding 跳过已存在结果。

---

# 13. Chunker 修复

## 13.1 核心目标

Chunker 必须安全处理中文和超长段落，不得死循环，不得 byte 切中文。

## 13.2 必须修复

* 区分目标长度和硬最大长度。
* 目标 chunk body 建议落在 500-800 字符。
* 硬最大 body 不超过 1000 字符。
* overlap 80-120 字符。
* overlap 不跨 section。
* 单个 block 超过硬最大时必须 Unicode char window 切分。
* 循环必须保证每轮都会推进 block 或消耗 long block。
* 上下文头可以单独计入 full content，但 body 限制必须明确。

## 13.3 验收标准

测试必须覆盖：

* 单个超长中文段落。
* overlap + next block 超过 target 时不会死循环。
* 多 section 时 overlap 不跨 section。
* body 长度不超过 hard max。
* document_summary 总是生成。
* section_summary 只在 sections 多于 1 时生成。
* 中文字符不会被 byte 切坏。

---

# 14. SSRF allowed_domains 修复

## 14.1 核心目标

source.allowed_domains 必须实际参与抓取安全判断。

## 14.2 必须实现

当 source.allowed_domains 非空时：

* URL hostname 必须属于允许域。
* 允许根域。
* 允许合法子域。
* 禁止相似域名绕过。
* 禁止 userinfo、fragment、特殊 hostname 解析绕过。

Fetcher 应尽量使用项目依赖中可靠的 URL parser。
如果仍存在 DNS rebinding / TOCTOU 边界，必须在注释和文档中明确说明，不得声称完全防护。

## 14.3 验收标准

测试必须覆盖：

* allowed root domain。
* allowed subdomain。
* evil-example.com 不允许冒充 example.com。
* example.com.evil.com 不允许。
* userinfo 绕过不允许。
* localhost / 127.0.0.1 / 私网 / metadata / link-local / multicast 仍被拒绝。
* redirect 仍不自动跟随。

---

# 15. DistillService 补强

## 15.1 核心目标

DistillService 必须保持与 Agent 主 LLM 隔离，并把网页正文视为非可信资料。

## 15.2 必须满足

* 使用 web_ingestion.distill_llm 配置。
* 不影响 AgentRuntime 主 LLM。
* api_key 为空给出清晰错误。
* 非法 JSON 重试一次。
* 429 / 5xx / timeout 有重试策略。
* 模型输出 Markdown fence 时可容错提取 JSON。
* prompt 明确网页正文不是指令，只是资料。
* token usage 缺失时不能 panic。

## 15.3 验收标准

测试必须覆盖：

* api_key 为空失败。
* 非法 JSON 重试。
* Markdown fence JSON 可解析。
* 429 / 5xx retry。
* AgentRuntime LLM 配置未被 distill_llm 污染。

---

# 16. Config 修复

## 16.1 核心目标

配置文件和环境变量必须一致，且默认安全。

## 16.2 必须补齐 env override

必须确保以下配置项的环境变量覆盖实际生效：

* web_ingestion.enabled
* scheduler_enabled
* dispatcher_enabled
* staging_required
* auto_publish_min_score
* pipeline_version
* scheduler_interval_secs
* dispatcher_interval_secs
* outbox_batch_size
* outbox_lock_ttl_secs
* retry_base_delay_secs
* retry_max_delay_secs
* distill_llm provider / base_url / model / api_key / temperature / top_p / timeout
* embedding provider / base_url / model / api_key / dimension / batch_size / timeout / collection

DEEPSEEK_API_KEY 只能作为 distill_llm api_key fallback，不能污染 embedding。

## 16.3 验收标准

测试必须覆盖：

* 每个新增 env override 生效。
* 默认 disabled。
* DEEPSEEK_API_KEY 只影响 distill_llm。
* embedding api_key 只影响 embedding。

---

# 17. 测试要求

禁止真实外网、真实 DeepSeek、真实 Qdrant、真实 MySQL。

可以使用：

* mock HTTP。
* mock LLM。
* mock embedding provider。
* mock Qdrant adapter。
* test transaction。
* repository mock，仅限测试，不得进入 production path。

必须补齐测试类型：

## 17.1 Outbox 测试

* duplicate event_key 幂等。
* SQL payload / last_error 特殊字符安全。
* claim / reclaim。
* locked_by 校验。
* retry / dead。
* 未实现 handler 不 mark published。

## 17.2 Pipeline 测试

* Scheduler 只创建 crawl_job。
* CrawlJobCreated 只发现当前 source URL。
* UrlDiscovered upsert page。
* unchanged skipped。
* changed 创建 run。
* PageFetched 后能继续进入下一阶段，或未实现时明确 failed。
* 任何 TODO handler 不得成功。

## 17.3 Publish / Rollback 测试

* publish 事务有效。
* Qdrant failure 阻止成功。
* supersede 旧版本 inactive。
* rollback 恢复目标版本。
* 同 page 双 active 不可能发生。

## 17.4 Retrieval 测试

* legacy 仍可召回。
* web_page_version staged 不可召回。
* web_page_version superseded 不可召回。
* web_page_version active published 可召回。
* keyword fallback 同样过滤。

## 17.5 Embedding / Chunker / SSRF 测试

* batch embedding。
* embedding 数量 / 维度校验。
* chunker 超长中文不死循环。
* allowed_domains 生效。
* 私网 / metadata 地址仍被拒绝。

---

# 18. 验收标准

本轮只有在以下全部满足时，才算完成：

1. 不存在返回成功的 TODO handler。
2. 未知 event 和未实现 event 都不会 mark published。
3. Outbox insert_event 真幂等。
4. Outbox raw SQL 全部安全参数化或等价安全实现。
5. Scheduler 与 CrawlJobCreatedHandler 职责不重复。
6. CrawlJobCreatedHandler 不跨 source。
7. UrlDiscoveredHandler 不 panic。
8. UrlDiscoveredHandler 会 upsert page。
9. latest_success_run_id 不会写 0。
10. run_key / version_key 使用当前配置 profile。
11. 状态机被所有 run 更新路径强制使用。
12. Publish 使用真实事务锁。
13. Publish 接入 Qdrant active 操作。
14. Qdrant 失败不会发布成功。
15. Rollback 接入 Qdrant active 操作。
16. RetrievalService 对 web_page_version 做 DB active 校验。
17. keyword fallback 也做 DB active 校验。
18. legacy RAG 不被破坏。
19. Web ingestion 真实写入现有知识库表。
20. Qdrant staged point 默认 active=false。
21. publish 后新 point active=true。
22. supersede 后旧 point inactive 或删除。
23. Embedding provider factory 生效。
24. OpenAI-compatible embedding provider 支持 batch、retry、数量校验、维度校验。
25. Chunker 不会死循环。
26. allowed_domains 生效。
27. env overrides 补齐。
28. config 默认仍 disabled。
29. cargo fmt 通过。
30. cargo check 通过。
31. cargo test 通过。
32. 测试报告明确列出新增测试覆盖项。

如果任一项未完成，必须如实列为未完成，不能写“Phase 完成”。

---

# 19. 最终输出格式

完成后输出：

1. 修改文件清单。
2. 是否有 SQL 变更；如果有，说明是否需要用户手动执行并重新生成 entity。
3. 每个 P0 修复项的完成状态。
4. 每个仍未完成项的明确说明。
5. 新增测试清单。
6. cargo fmt / cargo check / cargo test 结果。
7. web_ingestion 是否仍默认 disabled。
8. 是否可以安全开启 scheduler。
9. 是否可以安全开启 dispatcher。
10. 是否可以安全自动 publish。
11. Retrieval 是否已防止 staged / superseded web_ingestion 内容召回。
12. legacy RAG 是否有回归测试。

如果不能安全开启，必须明确写：

“当前仍不能开启 web_ingestion。”
