# Web Ingestion（知识摄入）深度解析

> 本文件专门详解 ServerRS 中最复杂的子系统 —— 知识摄入自动化流水线。
> 建议先阅读 `project-map.md` 了解项目全貌，再深入此模块。
> 最后核对：2026-06-28，基于当前 `src/app/web_ingestion`、`src/infra/web_ingestion`、`src/shared/config/web_ingestion.rs` 和 `scripts/web_ingestion`。

---

## 一、一句话概括

 **Web Ingestion** 是一个全自动的知识爬取与处理管道。它做的事情很简单：

> 给定一批网站来源（如新闻网站、百科网站），系统自动爬取网页 → 用 AI 理解内容 →
> 分块存储 → 发布到知识库 → AI 在聊天时就能用这些知识回答用户问题。

 整个过程无需人工干预（也可配置人工审核），类似给 AI 不断"喂"新知识。

---

## 二、核心文件一览

 ```
 src/
 ├── domain/web_ingestion/     ← 接口和数据结构定义（"合同"）
 │   ├── mod.rs
 │   ├── state_machine.rs      ★ 状态机（定义运行流程的合法状态转换）
 │   ├── status.rs             ★ 所有状态常量（23 种阶段状态）
 │   ├── event_types.rs        ★ 领域事件类型（18 种事件）
 │   ├── fetcher.rs            网页抓取接口
 │   ├── distiller.rs          LLM 蒸馏接口（AI 理解网页内容）
 │   ├── repository.rs         ★ 数据库操作接口（超长文件，21 KB）
 │   ├── review.rs             审核数据结构
 │   └── error.rs              错误类型定义
 │
 ├── app/web_ingestion/        ← 业务逻辑（"大脑"）
 │   ├── mod.rs
 │   ├── scheduler.rs          ★ 调度器（定时触发爬取任务）
 │   ├── dispatcher.rs         ★ 分发器（从事件队列取任务 -> 路由到处理器）
 │   ├── pipeline_context.rs   ★ 管道上下文（所有依赖的统一入口）
 │   ├── extractor.rs          ★ HTML→纯文本提取（去广告、去导航）
 │   ├── industrial_chunker.rs ★★ 工业级分块器（核心算法，20 KB）
 │   ├── quality_gate.rs       ★ 质量门控（评分决定自动发还是人工审）
 │   ├── review_service.rs     审核发布服务（管理员操作接口）
 │   ├── hash.rs               ★ 哈希计算（所有幂等键的生成）
 │   ├── state_machine_adapter.rs  状态机调用适配层
 │   ├── event_types.rs        事件类型常量重新导出
 │   ├── handlers/             ★★ 15 个事件处理器（每个文件处理一个阶段）
 │   │   ├── crawl_job_created.rs      爬取任务已创建
 │   │   ├── url_discovered.rs         ★ 最复杂的处理器（爬网页+创建运行记录）
 │   │   ├── page_fetched.rs           网页已抓取
 │   │   ├── page_cleaned.rs           网页已清洗
 │   │   ├── page_distilled.rs         网页已蒸馏（AI 理解）
 │   │   ├── quality_checked.rs        质量已检查
 │   │   ├── document_chunked.rs       ★ 文档已分块
 │   │   ├── chunks_embedded.rs        分块已向量化
 │   │   ├── document_indexed.rs       已建立索引
 │   │   ├── knowledge_staged.rs       已进入暂存区
 │   │   ├── publish_requested.rs      ★ 发布请求处理
 │   │   ├── rollback_requested.rs     回滚请求处理
 │   │   ├── terminal.rs               终端事件（已发布/已跳过等）
 │   │   └── unimplemented.rs          未实现占位
 │   └── services/             辅助服务
 │       ├── run_profile.rs           管道运行配置（版本信息）
 │       ├── run_key_builder.rs        运行键生成器
 │       ├── due_url_selector.rs       到期 URL 选择器
 │       ├── html_cleaner.rs           HTML 清洗
 │       ├── quality_result.rs         质量检查结果
 │       ├── qdrant_activation_service.rs  Qdrant 激活/停用
 │       ├── artifact_service.rs       中间产物持久化
 │       └── terminal_events.rs        终端事件发射
 │
 └── infra/web_ingestion/      ← 具体实现（"干活的"）
     ├── mod.rs
     ├── fetcher.rs            ★★ 网页抓取（SSRF 防护 + 限速，23 KB）
     ├── distiller.rs          ★ LLM 蒸馏（调 DeepSeek/Ollama 理解网页）
     ├── repositories.rs       ★★ 数据库操作（所有表操作，60 KB 超大文件）
     └── review_repository.rs       审核相关数据库操作（13 KB）
 ```

 **数据库表（15 张）：**
 ```
 web_sources                 来源配置（种子 URL、域名、审批状态）
 web_source_urls             从来源发现的 URL（调度队列）
web_pages                   网页实体索引（URL、hash、latest run 指针；正文不在这里）
 web_crawl_jobs              爬取任务
 knowledge_ingestion_runs    摄入运行记录（每次处理的完整追踪）
 knowledge_documents         处理后的文档
 knowledge_chunks            文档被切成的文本块
knowledge_embeddings        向量嵌入 JSON（DB 持久化，Qdrant 索引的数据来源）
 knowledge_publish_records   发布版本记录
 knowledge_chunk_manifests   分块版本映射
 knowledge_vector_manifests  向量版本映射
 domain_event_outbox         事件发件箱（异步驱动核心）
 web_ingestion_audit_logs    审计日志
 vector_index_jobs           向量索引任务
 vector_index_records        向量索引记录
 ```

---

## 三、整体架构图

```
┌──────────────────────────────────────────────────────────────────────────────┐
│                         Web Ingestion Architecture Overview                  │
└──────────────────────────────────────────────────────────────────────────────┘

                         ┌──────────────────┐
                         │    Scheduler     │  ← Timer: default interval is
                         │                  │     every 300 seconds
                         │                  │     Iterates over all enabled
                         │                  │     sources
                         └────────┬─────────┘
                                  │
                                  │ Creates crawl jobs and emits
                                  │ CrawlJobCreated events
                                  ▼
                         ┌──────────────────┐
                         │      Outbox      │  ★ Event outbox
                         │                  │    MySQL-backed event queue
                         │                  │    Ensures reliable async
                         │                  │    processing
                         └────────┬─────────┘
                                  │
                                  │ Dispatcher polls every 5 seconds by default
                                  │ Claims events by stage priority and handler
                                  │ capacity
                                  ▼
                  ┌──────────────────────────────────────┐
                  │             Dispatcher               │
                  │                                      │
                  │  Routes events to the corresponding  │
                  │  Handler based on event type         │
                  └───────────────┬──────────────────────┘
                                  │
                ┌─────────────────┼─────────────────┐
                ▼                 ▼                 ▼
        ┌──────────────┐  ┌──────────────┐  ┌──────────────────┐
        │ Handler 1:   │  │ Handler 2:   │  │ Handler N:       │
        │ URL Discovery│  │ Page         │  │ Publish &        │
        │ + Crawling   │  │ Distillation │  │ Activation       │
        │              │  │ (AI)         │  │ Qdrant Sync      │
        └──────┬───────┘  └──────┬───────┘  └────────┬─────────┘
               │                 │                   │
               └─────────────────┴───────────────────┘
                                  │
                                  │ Each completed step writes a new
                                  │ Outbox event
                                  ▼
                         ┌──────────────────┐
                         │  Ingestion Run   │  ★ One complete ingestion
                         │                  │    execution
                         │                  │    State machine:
                         │                  │    pending → published
                         │                  │    23 stages
                         └──────────────────┘

执行模型：事件驱动架构

  - 每个事件都会持久化写入 MySQL 的 domain_event_outbox 表。

  - Dispatcher 按固定间隔轮询，根据 Handler 容量和阶段优先级领取事件，
    然后并发路由到对应的 Handler。

  - Handler 完成处理后，会向 Outbox 写入新的事件，
    推动工作流进入下一个阶段。

  - 幂等性通过每个事件唯一的 event_key 保证，
    event_key 使用 SHA-256 生成。

  - 竞争保护：
      - 运行阶段流转使用 SQL 原子比较并交换（CAS）。
      - Outbox 事件在处理期间会续租锁。

  - 重试策略：
      - 失败事件使用指数退避进行重试。
      - 最大重试次数：5 次。
```

---

## 四、状态机详解 —— 一次"摄入运行"的完整生命周期

 每个 URL 从被发现到发布，会经历一条严格的状态转换链。

 ```
 状态链（14 步主路径）：

 pending ──► fetching ──► fetched ──► cleaning ──► cleaned ──► distilling
   (等待)     (正在抓)    (已抓完)    (正在清洗)    (已洗完)    (AI 理解中)
      │
      ▼
 distilling ──► distilled ──► quality_checked ──► chunking ──► chunked
   (AI 理解中)   (已理解完)     (质量已检查)        (正在分块)   (已分完块)
                     │
                     ▼
 chunked ──► embedding ──► embedded ──► indexing ──► indexed ──► staging
 (已分完块)  (向量化中)    (已向量化)    (建索引中)   (已建索引)   (待暂存)
                     │
                     ▼
 staging ──► publishing ──► published（最终状态）
 (待暂存)    (发布中)        (已发布 ✓)


 分支路径：
 - fetched ──► unchanged（内容无变化 → 跳过）
 - quality_checked ──► rejected（质量不合格 → 拒绝）
 - 任意阶段 ──► failed（出错）
 - 任意阶段 ──► dead（重试耗尽）
 ```

 **状态机代码位置**：[`src/domain/web_ingestion/state_machine.rs`](/src/domain/web_ingestion/state_machine.rs)

 所有合法的状态转换都硬编码在 `can_transition_run()` 函数中。非法转换（如跳过抓取直接蒸馏）会返回 `false`。

---

## 五、完整流水线 —— 15 个阶段逐段详解

> 以下按处理顺序，说明每个阶段做什么、谁处理、会产生什么结果。

---

### 第 0 步：Scheduler（调度器）

 | 属性 | 说明 |
 |------|------|
 | 代码 | `src/app/web_ingestion/scheduler.rs` |
| 触发方式 | 定时循环（代码默认每 300 秒 = 5 分钟；以配置文件为准） |
 | 干什么 | 遍历所有已启用的 `web_sources`，为每个来源创建一个 `web_crawl_job` |
 | 产出 | 发射 `CrawlJobCreated` 事件到 Outbox |

---

### 第 1 步：CrawlJobCreated → UrlDiscovered（URL 发现 + 抓取）

 | 属性 | 说明 |
 |------|------|
 | 处理器 | `handlers/url_discovered.rs`（**最复杂的处理器**） |
 | 代码量 | 14,869 字节 |
 | 干什么 | 获取一个爬取任务 → 从来源的种子 URL 开始 → **真正用 HTTP 抓取网页** |

 **详细流程：**

 ```
 ① 从事件中获取 source_url_id，从数据库读取 URL 和来源配置
 ② 校验来源是否启用、是否删除
 ③ 校验 URL 域名是否在来源的 allowed_domains 白名单中
 ④ 调用 Fetcher 抓取网页（带 SSRF 防护 + 限速）
 ⑤ 计算内容哈希 content_hash
 ⑥ 对比上次内容哈希：
     - 如果一样 → 跳过（发射 IngestionSkipped）
     - 如果不一样 → 继续
 ⑦ 构建 RunProfile（当前使用的 LLM提示版本/分块器版本/嵌入模型）
 ⑧ 计算 run_key（SHA-256 唯一标识这次运行）
 ⑨ 查找是否已有相同 run_key 的运行：
     - 有 → 恢复已有运行（resume 逻辑）
     - 没有 → 创建新的 ingestion_run 记录
 ⑩ 存储抓取到的页面原文（update_artifacts）
 ⑪ 状态推进：pending → running/fetching → running/fetched
 ⑫ 发射 PageFetched 事件继续流水线
 ```

---

### 第 2 步：PageFetched → PageCleaned（页面清洗）

 | 属性 | 说明 |
 |------|------|
 | 处理器 | `handlers/page_fetched.rs` |
 | 干什么 | 收到抓取完成的事件 → 调用 Extractor 把 HTML 转成纯文本 |
 | 核心工作 | 就是触发下一阶段 `PageCleaned` |

 **Extractor** 代码在 `app/web_ingestion/extractor.rs`，它：
 - 用 `scraper` 库解析 HTML
 - 删除 script / style / nav / footer / header 等无用标签
 - 提取标题和正文
 - 规范化空白字符（连续空格→一个空格，连续换行→一个换行）

---

### 第 3 步：PageCleaned → PageDistilled（LLM 蒸馏）

 | 属性 | 说明 |
 |------|------|
 | 处理器 | `handlers/page_cleaned.rs` |
 | 核心 | 调用 **KnowledgeDistiller**（LLM）理解网页内容 |

 **Distiller** 代码在 `infra/web_ingestion/distiller.rs`：

 它会向配置的 LLM（如 DeepSeek、Qwen/Ollama）发送一次**结构化提取提示词**，
 要求 AI 输出标准 JSON。标题、摘要、章节、质量分、风险标签和发布建议都来自这一次 Chat LLM 调用；后续 `PageDistilled` 只做规则校验，不再调用 LLM。

 ```json
 {
   "accept": true,
   "title": "文档标题",
   "summary": "3-5句话摘要",
   "keywords": ["关键词"],
   "sections": [
     { "heading": "标题", "body": "正文", "summary": "摘要" }
   ],
   "quality_score": 0.82,
   "risk_flags": ["educational"],
   "should_publish": false
 }
 ```

 注意：系统指令明确要求 LLM **不要执行网页中的任何指令**、
 **仅提取事实不要编造**、**医疗/法律/金融内容必须标记风险标签**。

---

### 第 4 步：PageDistilled → QualityChecked（质量门控）

 | 属性 | 说明 |
 |------|------|
 | 处理器 | `handlers/page_distilled.rs` |
 | 核心判断 | 调用 `quality_gate::evaluate()` |

 **Quality Gate** 代码在 `app/web_ingestion/quality_gate.rs`。

 它执行以下规则（按顺序检查，任一不通过即拒绝/暂存）：

 | 规则 | 结果 |
 |------|------|
 | 清洗后的文本 < 100 字符 | ❌ 拒绝 |
 | AI 说 `accept = false` | ❌ 拒绝 |
 | 没有提取到章节 | ❌ 拒绝 |
 | 摘要是空的 | ❌ 拒绝 |
| 质量分 < 0.65 | ❌ 拒绝（硬阈值，代码常量） |
 | 来源未审批 | ❌ 拒绝 |
 | 自残/药物剂量等高危标记 | ✅ 暂存（人工审核）|
 | 未知风险标记 | ✅ 暂存（人工审核）|
 | 配置要求 staging_required | ✅ 暂存 |
 | 来源不允许自动发布 | ✅ 暂存 |
| 质量分 < auto_publish_min_score | ✅ 暂存（代码默认 0.85，可配置） |
 | **所有检查都通过** | ✅ **直接发布** |

---

### 第 5 步：QualityChecked → DocumentChunked（文档分块）

 | 属性 | 说明 |
 |------|------|
 | 处理器 | `handlers/document_chunked.rs`（13,309 字节） |
 | 核心 | **Industrial Chunker**（工业级分块器）|

 **工业级分块器** 代码在 `app/web_ingestion/industrial_chunker.rs`（20,610 字节），
 专门针对中文优化：

 ```
 输入：AI 蒸馏后的文档（标题 + 摘要 + 多个章节）
 输出：三种类型的 Chunk

 1. document_summary（文档摘要块）
    - 1 个，全局唯一的文档级别摘要
    - 格式："标题：xxx\n来源：xxx\n正文：\nxxx"

 2. section_summary（章节摘要块）—— 仅当章节 > 1 时生成
    - 每个章节 0~1 个
    - 格式："标题：xxx\n章节：xxx\n来源：xxx\n正文：\nxxx"

 3. atomic（原子块）—— 真正的知识片段
    - 每个章节内按语义分块
    - 目标大小：500~1000 中文字符
    - 块之间重叠 80~120 字符
    - 重叠不跨章节边界
    - 超大段落（>1000 字符）按句号/问号等自然边界切分
 ```

 每个 Chunk 会生成一个**确定性 Chunk Hash**（SHA-256），
 保证同样的内容、同样的版本只会产生同样的 Chunk Hash，**天然幂等**。

---

### 第 6 步：DocumentChunked → ChunksEmbedded（向量化）

 | 属性 | 说明 |
 |------|------|
 | 处理器 | `handlers/chunks_embedded.rs` |
 | 干什么 | 给每个 Chunk 生成向量嵌入（Embedding）|

使用 `[embedding]` 配置的 Embedding 模型把文本块变成向量。`OllamaEmbeddingProvider` 会把 `[embedding].dimension` 作为 `dimensions` 请求字段发送，并校验返回维度。
向量元数据写入 `knowledge_chunks`，向量 JSON 写入 `knowledge_embeddings`；后续 Qdrant 索引从 DB 中读取这份向量。

---

### 第 7 步：ChunksEmbedded → DocumentIndexed（索引建立）

 | 属性 | 说明 |
 |------|------|
 | 处理器 | `handlers/document_indexed.rs` |
 | 干什么 | 把向量写入 Qdrant 向量数据库（如果启用了 Qdrant）|

 Qdrant 的 Point ID 是确定性生成的（SHA-256），保证同样的 chunk 不会重复写入。

---

### 第 8 步：DocumentIndexed → KnowledgeStaged（存入暂存区）

 | 属性 | 说明 |
 |------|------|
 | 处理器 | `handlers/document_indexed.rs`（后半段） |
 | 干什么 | 创建 `knowledge_publish_record`，状态 = staged |

 此时数据已在数据库中，但处于**暂存状态**（`status=0`），
 用户的检索请求**查不到**这些数据。需要发布后才能被检索到。

---

### 第 9 步：KnowledgeStaged → PublishRequested（发布请求）

 | 属性 | 说明 |
 |------|------|
 | 处理器 | `handlers/knowledge_staged.rs` |
 | 触发 | 如果 Quality Gate 判定为 Publishable + auto_publish=true，则自动发射发布请求 |
 | 人工触发 | 管理员通过 `POST /api/v1/admin/web-ingestion/reviews/{id}/publish` 手动触发 |

---

### 第 10 步：PublishRequested → Published（正式发布）

 | 属性 | 说明 |
 |------|------|
 | 处理器 | `handlers/publish_requested.rs`（10,242 字节）|
| 最重要的事 | **事务性发布**：替换旧版本 + 激活新版本 |

 **原子操作：**
 1. `publish_in_tx()`：在一个 MySQL 事务内完成
    - 将同来源/同页面的旧版本标记为 `superseded`（被替代）
    - 将新版本标记为 `active`（当前活跃版本）
 2. **Qdrant 同步**：更新 Qdrant 中的 active 标记（DB 是权威，Qdrant 尽力同步）
 3. 更新文档状态：`knowledge_documents.status` 从 0 → 1（可检索）
 4. 状态推进：staging → running/publishing → published/published（最终态）

---

### 可选步骤：Rollback（回滚）

 | 属性 | 说明 |
 |------|------|
 | 处理器 | `handlers/rollback_requested.rs` |
 | 触发 | 管理员手动请求回滚到旧版本 |

 回滚时：
 1. 把当前活跃版本标记为 `rolled_back`
 2. 把目标旧版本重新标记为 `active`
 3. 同步 Qdrant 状态

---

## 六、幂等性设计 —— 为什么事件重复处理不会出问题？

 整个流水线最精巧的设计就是**全链路幂等**：

### 6.1 事件级别幂等

 每个 Outbox 事件都有一个 **event_key**（SHA-256 哈希）：
 ```
 event_key = sha256(event_type | aggregate_type | aggregate_id | run_id | version_key)
 ```
 数据库有 UNIQUE 约束，重复插入直接忽略。

### 6.2 运行级别幂等

 每个摄入运行有一个 **run_key**（SHA-256 哈希）：
 ```
 run_key = sha256(source_id | page_id | content_hash | llm_prompt_version |
                  chunker_version | embedding_model | pipeline_version)
 ```
 同样的来源、同样的内容、同样的管道版本 → 同样的 run_key。
 如果发现已存在的 run_key，不是直接跳过，而是**检查运行状态并恢复**（resume 逻辑）。

### 6.3 Chunk 级别幂等

 每个 Chunk 有一个 **chunk_hash**（SHA-256 哈希）：
 ```
 chunk_hash = sha256(version_key | chunk_type | chunk_index | content | chunker_version)
 ```
 同样的 chunk_hash 不会重复插入。

### 6.4 状态机级别幂等

 状态转换函数 `can_transition_run()` 明确包含一条规则：
 ```
 if from_status == to_status && from_stage == to_stage → true（幂等）
 ```
 到达同一状态不视为错误。

 运行表的阶段推进不是“先查再改”，而是单条 SQL CAS：
 ```sql
 UPDATE knowledge_ingestion_runs
 SET status = ?, stage = ?
 WHERE id = ? AND status = ? AND stage = ?
 ```

 因此多个 dispatcher 或多个主机实例同时处理同一个 run 时，只有一个 worker 会得到 `rows_affected = 1` 并继续发下游事件；其他 worker 会看到状态已被推进，按幂等重放处理。

### 6.5 Outbox 锁与恢复

 Dispatcher 领取事件时在事务内使用 `FOR UPDATE SKIP LOCKED` 锁定候选行，再写入 `locked_by/locked_until`。长耗时 handler（蒸馏、embedding、Qdrant upsert 等）执行期间会周期性续租 `locked_until`。

 如果进程或主机崩溃，续租停止；`locked_until` 过期后，其他实例可以重新领取事件。handler 会根据 run 当前阶段和已落库 artifact 做 resume，因此重复事件不会直接造成重复发布。

---

## 七、SSRF 防护 —— 网页抓取的安全性

 **代码位置**：`src/infra/web_ingestion/fetcher.rs`（23,045 字节）

 网页抓取是一个高风险操作，代码实现了多层防护：

### 7.1 协议限制
 - 只允许 `http://` 和 `https://`
 - 不允许带用户信息（`user:password@`）

### 7.2 IP 黑名单（SSRF 防护）
 抓取前先 DNS 解析，然后检查 IP 地址：

 | 禁止的 IP 范围 | 原因 |
 |---|---|
 | 127.0.0.0/8 | 本机回环 |
 | 10.0.0.0/8 | 内网 |
 | 172.16.0.0/12 | 内网 |
 | 192.168.0.0/16 | 内网 |
 | 169.254.x.x | 云元数据 API（如 AWS 的 169.254.169.254）|
 | 224.0.0.0~239.0.0.0 | 组播 |
 | 240.0.0.0+ | 保留 |
 | IPv6 loopback/link-local | 同上 |

### 7.3 域名白名单
 每个来源可以配置 `allowed_domains`，只允许抓取指定域名的 URL。
 白名单使用后缀匹配：`sub.example.com` 匹配 `.example.com`。

### 7.4 请求限速
 - 每个域名有最小请求间隔（默认 2 秒）
 - 增加随机抖动（默认 ±1 秒）
 - 429/503 状态码自动等待 Retry-After

### 7.5 内容限制
 - 只允许 HTML/XML/纯文本的 Content-Type
 - 有最大 body 大小限制
 - 最多跟随 5 次重定向

---

## 八、配置详解

在 `config.toml` 的 `[web_ingestion]` 段。下面是可运行配置示例；代码默认值更保守：`enabled=false`、`scheduler_enabled=false`、`dispatcher_enabled=false`、`auto_publish=false`、`staging_required=true`。

 ```toml
 [web_ingestion]
# ★★ 总开关 —— false 时 scheduler/dispatcher 都不会启动
enabled = true

 # ★★ 调度器开关 —— 定时爬取新内容
 scheduler_enabled = true

 # ★★ 分发器开关 —— 处理事件队列
 dispatcher_enabled = true

# ★ 发布模式
# staging_required = true  → 所有内容都进暂存区，人工审核后才能发布
# auto_publish = true      → 质量达标的内容自动发布
staging_required = false
auto_publish = true
auto_publish_min_score = 0.85   # 自动发布最低质量分；低于 0.65 会硬拒绝

# 版本标识（用于 run_key 计算，改了这个值会重新处理所有内容）
pipeline_version = "20260612"
llm_prompt_version = "20260612_v1"
chunker_version = "20260612"

# HTTP 抓取配置
fetch_timeout_secs = 30
fetch_user_agent = "ServerRSKnowledgeBot/0.1"
fetch_proxy_url = "http://127.0.0.1:7890"   # 代理地址
max_body_bytes = 5242880                      # 最大 5MB

 # 请求限速
min_request_interval_ms = 2000   # 每个域名最小间隔 2 秒
request_jitter_ms = 1000        # 随机抖动 ±1 秒
max_urls_per_source_per_job = 20
url_enqueue_dedupe_secs = 86400

# 分块参数
chunk_target_min = 500           # 目标最小分块大小（字符）
chunk_target_max = 1000          # 目标最大分块大小（字符）
 chunk_overlap_min = 80           # 块重叠最小字符
 chunk_overlap_max = 120          # 块重叠最大字符

# 调度/分发
scheduler_interval_secs = 300    # 代码默认 5 分钟；批量导入后可临时调小
dispatcher_interval_secs = 5
outbox_batch_size = 20           # 每轮最多领取的 outbox 事件总数上限
dispatcher_parallelism = 8       # 每轮并发上限；总领取数 <= min(outbox_batch_size, dispatcher_parallelism)
outbox_lock_ttl_secs = 300       # 事件锁 TTL；处理期间会自动续租
retry_base_delay_secs = 30
retry_max_delay_secs = 1800
embedding_batch_size = 32
qdrant_collection = "web_ingestion"

[web_ingestion.handler_parallelism]
default = 1
crawl_job_created = 1
url_discovered = 4               # 抓取阶段；同 host 仍受 min_request_interval_ms 限速
page_fetched = 6                 # HTML 清洗，CPU/内存轻中等
page_cleaned = 2                 # LLM 蒸馏，建议保守
page_distilled = 4
quality_checked = 4
document_chunked = 2
chunks_embedded = 2              # embedding provider 压力较大，建议保守
document_indexed = 2
knowledge_staged = 2
knowledge_publish_requested = 1  # 发布/激活建议单 worker
knowledge_rollback_requested = 1
terminal = 8

# LLM 蒸馏配置（独立于聊天 LLM）
[web_ingestion.distill_llm]
provider = "deepseek"
base_url = "https://api.deepseek.com/v1"
chat_model = "deepseek-chat"
# 注意：DEEPSEEK_API_KEY 环境变量会自动注入到这里
api_key = ""
temperature = 0.1                # 蒸馏用低温度，确保输出稳定
top_p = 0.9
timeout_secs = 120
```

Dispatcher 领取 outbox 事件时不是简单 `ORDER BY priority LIMIT N`。当前实现会先按流水线深度排序，再按 `[web_ingestion.handler_parallelism]` 给每个事件类型组分配领取配额。例如 `page_cleaned = 2` 时，一轮最多锁定 2 条 `PageCleaned` 事件，剩余总并发容量会留给其他阶段，避免慢阶段把大量事件锁在内存里等待 worker。

---

## 九、种子脚本与导入方式

当前脚本集中在 `scripts/web_ingestion/`：

| 脚本 | 作用 |
|---|---|
| `1.create-tasks.ps1` | 批量创建中文维基百科多主题采集任务：调用 `2.fetch-urls.ps1` 导出 URL，再调用 `3.import-urls.ps1` 导入数据库 |
| `2.fetch-urls.ps1` | 按 Wikipedia category 拉取页面 URL，支持 `-MaxPages`、`-MaxDepth`、`-ProxyUrl`、`-DelayMs` |
| `3.import-urls.ps1` | 创建/更新 `web_sources`，批量导入 `web_source_urls` |
| `publish-reviewed-web-knowledge.ps1` | 给已审核的 `knowledge_publish_records.id` 写入发布事件，由 ServerRS 正常发布 |

常用命令：

```powershell
pwsh -File .\scripts\web_ingestion\1.create-tasks.ps1 `
  -UserAgent "ServerRSKnowledgeBot/0.1 (mailto:you@example.com)" `
  -Groups All `
  -MaxPagesPerTopic 300 `
  -CategoryDepth 1 `
  -ApiDelayMs 1000 `
  -Parallelism 8
```

只导出 URL 文件和 manifest，不写数据库：

```powershell
pwsh -File .\scripts\web_ingestion\1.create-tasks.ps1 `
  -UserAgent "ServerRSKnowledgeBot/0.1 (mailto:you@example.com)" `
  -Groups All `
  -ExportOnly
```

`3.import-urls.ps1` 支持两种数据库连接方式：

```powershell
# Docker 容器方式（默认容器名 serverrs-mysql）
pwsh -File .\scripts\web_ingestion\3.import-urls.ps1 `
  -SourceName "zhwiki-biology" `
  -UrlFile ".\data\seed\wikipedia\zhwiki-biology.txt" `
  -AllowedDomains "zh.wikipedia.org"

# 直连数据库方式
pwsh -File .\scripts\web_ingestion\3.import-urls.ps1 `
  -SourceName "zhwiki-biology" `
  -UrlFile ".\data\seed\wikipedia\zhwiki-biology.txt" `
  -AllowedDomains "zh.wikipedia.org" `
  -DatabaseUrl "mysql://root:passwd@127.0.0.1:3306/digital_companion"
```

导入脚本只负责 `web_sources` 和 `web_source_urls`。真正的抓取、蒸馏、分块、向量化、发布都由 ServerRS 启动后的 scheduler/dispatcher 处理。

---

## 十、审核与管理接口

管理员通过以下 API 操作知识摄入审核流程：

### 10.1 查看待审核列表

 ```
 GET /api/v1/admin/web-ingestion/reviews?publish_status=staged&page=1&page_size=20
 ```

 返回每个待审核条目的摘要信息。

### 10.2 查看详情

 ```
 GET /api/v1/admin/web-ingestion/reviews/{publish_record_id}
 ```

 返回完整信息：原文内容、AI 蒸馏结果、审计日志。

### 10.3 通过审核（发布）

 ```
 POST /api/v1/admin/web-ingestion/reviews/{publish_record_id}/publish
 Body: { "notes": "审核通过，内容准确" }
 ```

 如果分发器未启用，返回 409 Conflict。

### 10.4 审核显示字段

 审核页显示每个条目的：
 - 来源名称
 - 来源 URL
 - 标题
 - 当前发布状态（staged / published / superseded）
 - 质量评分
 - AI 生成的 should_publish 建议
 - 风险标记
 - 创建时间 / 更新时间

---

## 十一、数据生命周期图

 以下是一段知识从网页到 AI 回答的完整旅程：

 ```
 1. 管理员配置来源
    web_sources 表: { id:1, seed_urls:["https://xxx.com/rss"],
                     allowed_domains:["xxx.com"], approved }

2. Scheduler 按 `scheduler_interval_secs` 定时触发
    → 创建 crawl_job ( web_crawl_jobs )
    → 发射 CrawlJobCreated 事件到 outbox

 3. Dispatcher 取到事件
    → 路由到 url_discovered handler
    → 使用来源种子 URL 发现待抓 URL
    → URL 写入 web_source_urls 表
    → 发射 UrlDiscovered 事件

 4. Dispatcher 再次处理 UrlDiscovered 事件
    → HTTP 抓取网页（SSRF 防护，限速）
    → 计算 content_hash 比对上次
    → 如果内容有变化：
       → 创建 ingestion_run（knowledge_ingestion_runs）
       → 创建 web_page（web_pages）
       → 存储原始 HTML（update_artifacts）
       → 发射 PageFetched 事件

 5. PageFetched → Extractor 清洗 HTML
    → 发射 PageCleaned 事件

 6. PageCleaned → Distiller 用 LLM 理解
    → LLM 输出 JSON（标题、摘要、章节、质量分、风险标记）
    → 发射 PageDistilled 事件

 7. PageDistilled → Quality Gate 打分
    → 决定：拒绝 / 暂存 / 自动发布
    → 发射 QualityChecked 事件

 8. QualityChecked → 工业级分块器
    → 产生 document_summary + section_summary + atomic chunks
    → 存入 knowledge_chunks 表（status=0）
    → 创建 publish_record（status=staged）
    → 发射 DocumentChunked 事件

 9. DocumentChunked → ChunksEmbedded：向量化每个 Chunk
    → 存入 knowledge_embeddings 表
    → 发射 ChunksEmbedded 事件

 10. ChunksEmbedded → DocumentIndexed：建立向量索引
     → 写入 Qdrant 向量库（如果启用）
     → 发射 DocumentIndexed 事件

 11. DocumentIndexed → KnowledgeStaged：进入暂存区
     → 创建 publish_record（active=0）
     → 发射 KnowledgeStaged 事件

 12. KnowledgeStaged → 自动（或手动）发布
    → publish_in_tx: 替换旧版 + 激活新版
     → knowledge_documents.status = 0 → 1（可检索）
     → Qdrant active 标记更新
     → 发射 KnowledgePublished 事件

 13. ✅ 用户聊天时，AI 通过 RAG 检索到这段知识

 14. ♻️ 下次同一 URL 被抓取 → 内容哈希比对
     - 相同的 SHA-256 → 跳过（IngestionSkipped）
     - 不同的 SHA-256 → 重复 4~12 步，新版替代旧版
 ```

---

## 十二、关键设计决策

### 12.1 为什么用事件驱动而不是同步管道？

 因为每个阶段耗时不同（抓取 3 秒 vs AI 蒸馏 30 秒 vs 分块 0.1 秒）。
 同步执行会让快的阶段等慢的阶段，浪费资源。事件驱动允许：
 - 不同的 URL 处于不同阶段，并行处理
 - 失败后自动重试，不影响其他 URL
 - 断开后可以从断点继续（resume 逻辑）

### 12.2 Outbox 模式（事件发件箱）是什么？

 保证"操作数据库"和"发事件"是**原子**的。流程：
 1. Handler 先写入业务数据（如创建 ingestion_run）
 2. 然后写入一条 outbox 事件（在同一数据库事务中）
3. Dispatcher 从 outbox 按阶段配额领取事件，分发处理
4. 处理成功 → 标记 outbox 事件为 published

Dispatcher 领取事件时会按流水线深度排序：越接近发布的事件优先级越高，其次才是 `PageFetched`、`UrlDiscovered` 和 `CrawlJobCreated`，避免大规模种子导入时只抓新 URL、不推进已抓页面。

同时，领取动作本身会遵守 `[web_ingestion.handler_parallelism]`。仓库层提供 `claim_batch_by_quotas`，Dispatcher 会把每个事件类型组转换成配额后再领取：

```text
本轮总上限 = min(outbox_batch_size, dispatcher_parallelism)
PageCleaned 领取上限 = handler_parallelism.page_cleaned
ChunksEmbedded 领取上限 = handler_parallelism.chunks_embedded
...
```

这意味着慢阶段不会一次锁住超过自身 worker 数的事件。例如 `page_cleaned = 2`，即使 outbox 里有大量 `PageCleaned`，本轮也只会领取 2 条，其他容量会继续分配给后续或其他可运行阶段。多实例部署时也能减少"某个实例提前锁住但还没开始处理"造成的吞吐浪费。

如果 Dispatcher 在处理中间崩溃了，事件还在 outbox 表里；锁续租停止后，`locked_until` 到期，重启后的实例或其他主机实例会继续处理。

### 12.3 为什么要有 Qdrant 同步？DB 不是也有数据吗？

 DB 是权威数据源（source of truth），Qdrant 是加速索引。
 - RetrievalService 检索时**先查 DB 确认 status=1**
 - 再从 Qdrant 做语义相似度搜索
 - 即使 Qdrant 数据过期，DB 的 status 也能兜底

### 12.4 run_key 到底有什么作用？

 run_key 是**内容 + 管道版本**的哈希。它保证：
 - 同一篇内容，用同样的管道版本处理 → 只处理一次（幂等）
 - 更新了管道版本（如换了更好的分块器）→ 重新处理所有内容
 - 内容更新了 → 自动触发新版本的摄入运行

---

## 十三、流水线涉及的核心数据库表详解

### 13.1 来源管理（3 张表）

 **web_sources** —— 要爬取的网站配置
 ```sql
 id BIGINT PRIMARY KEY,
 name VARCHAR(100),            -- 来源名称
 source_type VARCHAR(20),      -- rss / web / api
 seed_urls JSON,                -- 种子 URL 列表
 allowed_domains JSON,          -- 允许抓取的域名白名单
 approval_status VARCHAR(20),   -- pending / approved / rejected / disabled
 trust_level VARCHAR(20),       -- official / trusted / normal / untrusted
 auto_publish BOOLEAN,          -- 是否允许自动发布
 crawl_interval_minutes INT,    -- 爬取间隔
 enabled BOOLEAN,               -- 是否启用
 deleted_at DATETIME,
 ```

 **web_source_urls** —— 从来源发现的待爬 URL
 ```sql
 id BIGINT PRIMARY KEY,
 source_id BIGINT,              -- 关联 web_sources
 url VARCHAR(2048),             -- URL
 url_hash CHAR(64),             -- SHA-256
 status VARCHAR(20),            -- discovered / queued / fetching / fetched / failed
 last_content_hash CHAR(64),    -- 上次抓取的内容哈希
 retry_count INT,
 enabled BOOLEAN,
 next_fetch_at DATETIME,
 deleted_at DATETIME,
 ```

 **web_pages** —— 抓取到的网页实体
 ```sql
 id BIGINT PRIMARY KEY,
 source_id BIGINT,
 source_url_id BIGINT,
 url VARCHAR(2048),
 canonical_url VARCHAR(2048),   -- 最终 URL（可能被重定向）
 url_hash CHAR(64),
 latest_content_hash CHAR(64),  -- 最新内容的哈希
 latest_success_run_id BIGINT,  -- 最近成功处理的运行 ID
 ```

### 13.2 运行与文档（4 张表）

 **knowledge_ingestion_runs** —— 一次摄入运行的完整追踪
 ```sql
 id BIGINT PRIMARY KEY,
 source_id BIGINT,
 source_url_id BIGINT,
 crawl_job_id BIGINT,
 page_id BIGINT,
 content_hash CHAR(64),
 content_key CHAR(64),
 run_key CHAR(64) UNIQUE,       -- ★ 幂等键
 version_key CHAR(64),
 status VARCHAR(20),             -- pending/running/staged/published/rejected...
 stage VARCHAR(20),              -- pending/fetching/fetched/cleaning/...
 last_error TEXT,
 error_count INT,
 quality_score DECIMAL(4,3),
 quality_result JSON,            -- 质量门控结果
 distilled_json JSON,            -- LLM 蒸馏结果
 embedding_provider VARCHAR(64),
 embedding_model VARCHAR(128),
 embedding_dimension INT,
 started_at DATETIME,
 finished_at DATETIME,
 ```

 **knowledge_documents** —— 处理后的文档
 ```sql
 document_id BIGINT PRIMARY KEY,
 source_type VARCHAR(32),        -- 'web_ingestion'（vs 传统 RAG）
 source_id BIGINT,               -- 关联 ingestion_run.id
 title VARCHAR(500),
 content_hash CHAR(64),
 metadata JSON,
 status TINYINT,                 -- 0=暂存, 1=已发布
 ```

**knowledge_chunks** —— 文档被切成的文本块
```sql
chunk_id BIGINT PRIMARY KEY,
document_id BIGINT,
chunk_index INT,
content TEXT,
token_count INT,
metadata JSON,
vector_id VARCHAR(128),          -- Qdrant point / 向量索引标识
embedding_provider VARCHAR(64),
embedding_model VARCHAR(128),
embedding_dimension INT,
status TINYINT,                 -- 0=暂存, 1=已发布
```

**knowledge_embeddings** —— Chunk 的 embedding JSON
```sql
embedding_id BIGINT PRIMARY KEY,
chunk_id BIGINT,
provider VARCHAR(64),
model VARCHAR(128),
dimension INT,
embedding_json JSON,
```

### 13.3 发布与版本（3 张表）

 **knowledge_publish_records** —— 版本发布记录
 ```sql
 id BIGINT PRIMARY KEY,
 source_id BIGINT,
 page_id BIGINT,
 run_id BIGINT,
 document_id BIGINT,
 version_key CHAR(64),
 content_hash CHAR(64),
 publish_status VARCHAR(32),     -- staged/publishing/published/superseded/rolled_back/failed
 active TINYINT(1),              -- 是否当前活跃版本
 active_page_key VARCHAR(128),   -- source_id:page_id（UNIQUE，保证每页只有1个活跃版）
 activated_at DATETIME,
 superseded_at DATETIME,
 superseded_by_record_id BIGINT, -- 被哪个新版本替代了
 rolled_back_from_record_id BIGINT,
 ```

 **knowledge_chunk_manifests** —— 发布版本→Chunk 的映射
 ```sql
 id BIGINT PRIMARY KEY,
 publish_record_id BIGINT,
 run_id BIGINT,
 document_id BIGINT,
 chunk_id BIGINT UNIQUE,
 version_key CHAR(64),
 chunk_hash CHAR(64),            -- 幂等键
 chunk_type VARCHAR(32),         -- document_summary / section_summary / atomic
 chunk_index INT,
 active TINYINT(1),
 ```

 **knowledge_vector_manifests** —— Chunk→Qdrant 向量的映射
 ```sql
 id BIGINT PRIMARY KEY,
 publish_record_id BIGINT,
 run_id BIGINT,
 document_id BIGINT,
 chunk_id BIGINT,
 chunk_hash CHAR(64),
 qdrant_collection VARCHAR(128),
 qdrant_point_id CHAR(64),       -- 确定性 Qdrant Point ID
 embedding_provider VARCHAR(64),
 embedding_model VARCHAR(128),
 embedding_dimension INT,
 active TINYINT(1),
 ```

### 13.4 事件与审计（2 张表）

 **domain_event_outbox** —— 异步事件驱动核心
 ```sql
 id BIGINT PRIMARY KEY,
 event_key CHAR(64) UNIQUE,      -- ★ 幂等键
 event_type VARCHAR(128),         -- 事件类型（18 种之一）
 aggregate_type VARCHAR(64),      -- 聚合类型
 aggregate_id BIGINT,
 payload JSON,                    -- 仅存 ID 和小元数据
 status VARCHAR(32),              -- pending/processing/published/failed/dead
 retry_count INT,
 max_retries INT,
 next_retry_at DATETIME,
 locked_by VARCHAR(128),
 locked_until DATETIME,
 last_error TEXT,
 published_at DATETIME,
 ```

 **web_ingestion_audit_logs** —— 审计日志
 ```sql
 id BIGINT PRIMARY KEY,
 source_id BIGINT,
 source_url_id BIGINT,
 page_id BIGINT,
 run_id BIGINT,
 publish_record_id BIGINT,
 action VARCHAR(64),              -- 操作类型（18 种）
 status VARCHAR(32),              -- info/warning/error/success
 message TEXT,
 metadata JSON,
 created_at DATETIME,
 ```

### 13.5 其他（2 张表）

 **web_crawl_jobs** —— 爬取任务
 ```sql
 id BIGINT PRIMARY KEY,
 source_id BIGINT,
 status VARCHAR(20),             -- pending/running/succeeded/failed/dead/cancelled
 scheduled_at DATETIME,
 started_at DATETIME,
 finished_at DATETIME,
 ```

 **vector_index_jobs / vector_index_records** —— 向量索引管理

---

## 十四、配置文件改动指南

 如果你想用 Web Ingestion 接入自己的数据源，需要改的地方：

### 新增一个数据源

 在 MySQL 中插入一条记录：
 ```sql
 INSERT INTO web_sources (name, source_type, seed_urls, allowed_domains,
                          approval_status, trust_level, auto_publish, enabled)
 VALUES ('MySource', 'web', '["https://mysource.com/rss"]',
         '["mysource.com"]', 'approved', 'trusted', true, true);
 ```

### 调整爬取频率

 ```toml
 [web_ingestion]
 scheduler_interval_secs = 3600    # 每小时检查一次新内容
 min_request_interval_ms = 5000    # 每个域名 5 秒才能抓一次
 ```

### 关闭自动发布，全部人工审核

 ```toml
 [web_ingestion]
 auto_publish = false
 staging_required = true
 ```

### 切换蒸馏用的 AI 模型

 ```toml
 [web_ingestion.distill_llm]
 # 改成用 DeepSeek
 provider = "deepseek"
 base_url = "https://api.deepseek.com/v1"
 chat_model = "deepseek-chat"
 # 通过环境变量注入：set DEEPSEEK_API_KEY=sk-xxx
 temperature = 0.1
 ```

### 使用两个 Ollama 做蒸馏负载均衡

Web Ingestion 的 `distill_llm.base_url` 只配置一个 OpenAI-compatible 入口。如果有两台 Ollama，推荐在 ServerRS 本机前置一个 Nginx，把两个 Ollama tunnel 合成一个本地入口：

```text
127.0.0.1:11111 -> SSH tunnel -> Ollama A :11434
127.0.0.1:11112 -> SSH tunnel -> Ollama B :11434
127.0.0.1:18080 -> Nginx -> 11111 / 11112
```

仓库内提供了示例配置：`nginx/ollama-distill-lb.conf`。启动/检查命令：

```powershell
nginx -p "d:\WorkSpace\ServerRS\nginx" -c "ollama-distill-lb.conf" -t
nginx -p "d:\WorkSpace\ServerRS\nginx" -c "ollama-distill-lb.conf"
curl http://127.0.0.1:18080/health
curl http://127.0.0.1:18080/v1/models
```

ServerRS 只需要指向这个本地入口：

```toml
[web_ingestion.distill_llm]
provider = "Ollama"
base_url = "http://127.0.0.1:18080/v1"
chat_model = "qwen3:14b"
temperature = 0.1
top_p = 0.9
timeout_secs = 180
```

要让两台 Ollama 都有活干，还需要把蒸馏阶段 worker 打开，例如：

```toml
[web_ingestion]
dispatcher_parallelism = 10
outbox_batch_size = 30

[web_ingestion.handler_parallelism]
page_cleaned = 2
```

### 全部重置（重新处理所有已抓取内容）

 ```toml
 [web_ingestion]
 pipeline_version = "20260615-new"  # 改这个值就行
 ```
 修改 `pipeline_version` 会改变所有内容的 run_key，导致所有内容被重新处理一次。

---

## 十五、与检索服务（RAG）的关系

 Web Ingestion 产出的知识和用户通过聊天上传的资料**共享同一个 RAG 检索服务**：

 ```
 RetrievalService
 ├── 通道 1: 用户上传的文档（传统 RAG）
 │   source_type != "web_ingestion"
 │   status = 1（直接发布）
 │
 └── 通道 2: Web Ingestion 自动爬取的知识
     source_type = "web_ingestion"
     status = 0（暂存）→ 发布后 status = 1
     Qdrant collection = 独立配置的 web 集合
 ```

 检索时，RetrievalService 会同时搜索两个通道，但**只返回 status=1 的内容**。

---

*最后核对时间：2026-06-27*
*基于当前工作区代码同步*
