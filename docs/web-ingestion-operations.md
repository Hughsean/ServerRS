# Web 知识采集运维手册

## 合规边界

- 维基百科：使用 MediaWiki API 发现 URL；机器人标识必须包含真实联系方式；
  请求应串行发送，并遵守 `429`、`503`、`Retry-After` 和 `maxlag`。
  大规模导入应优先使用 Wikimedia 数据转储，而不是逐页抓取 HTML。
- 百度百科：当前 `robots.txt` 包含 `User-agent: *` 和 `Disallow: /`。
  未获得书面许可或授权数据接口前，不要批量抓取。系统不支持轮换 IP、
  伪装浏览器 UA、绕过验证码等规避措施。
- 发布衍生知识时，应保留来源 URL，并遵守来源的许可证和署名要求。

## 安全默认配置

系统目前使用以下保守配置：

```toml
[web_ingestion]
fetch_user_agent = "ServerRSKnowledgeBot/0.1 (mailto:you@example.com)"
fetch_proxy_url = "http://127.0.0.1:7890"
min_request_interval_ms = 2000
request_jitter_ms = 1000
max_urls_per_source_per_job = 20
url_enqueue_dedupe_secs = 86400
scheduler_interval_secs = 900
```

正式运行前，必须将 `you@example.com` 替换为你的真实联系邮箱。

- 请求间隔按域名单独计算。
- fetcher 和批量维基脚本默认通过 `127.0.0.1:7890` 代理访问网络。
- 重定向产生的请求也会限速。
- 调度器每轮最多为一个来源投递 20 个到期 URL。
- 同一 URL 在 24 小时窗口内只会投递一次，避免队列积压时重复建任务。
- 除非目标网站明确允许并发，否则不要并行运行多个采集进程。

首次导入建议设置 `auto_publish = false`，先人工检查暂存知识的提取质量、
来源署名和内容风险，确认稳定后再启用自动发布。

## 规划通用知识来源

不要把全部知识混在一个来源中。建议按主题分别创建 `web_sources`，这样可以
单独控制抓取周期、停用异常来源，并追踪每类知识的质量。

可以从以下主题开始：

| 主题 | 维基百科分类示例 | 来源名称示例 |
| --- | --- | --- |
| 计算机科学 | `计算机科学` | `zhwiki-computer-science` |
| 人工智能 | `人工智能` | `zhwiki-artificial-intelligence` |
| 数学 | `数学` | `zhwiki-mathematics` |
| 物理学 | `物理学` | `zhwiki-physics` |
| 化学 | `化学` | `zhwiki-chemistry` |
| 生物学 | `生物学` | `zhwiki-biology` |
| 医学 | `医学` | `zhwiki-medicine` |
| 心理学 | `心理学` | `zhwiki-psychology` |
| 历史 | `历史` | `zhwiki-history` |
| 地理 | `地理学` | `zhwiki-geography` |
| 经济学 | `经济学` | `zhwiki-economics` |
| 哲学 | `哲学` | `zhwiki-philosophy` |
| 法律 | `法学` | `zhwiki-law` |
| 文学 | `文学` | `zhwiki-literature` |
| 艺术 | `艺术` | `zhwiki-art` |

分类名称需要与维基百科上的实际分类一致。建议先从每个主题 100 到 500 篇
开始，检查清洗、蒸馏、分块、向量化和检索效果，再逐步扩大。

## 准备维基百科任务

从一个维基百科分类中生成正文页面 URL 清单：

```powershell
pwsh -File .\scripts\fetch-wikipedia-category-urls.ps1 `
  -Category "人工智能" `
  -OutputFile .\data\seed\zhwiki-artificial-intelligence.txt `
  -UserAgent "ServerRSKnowledgeBot/0.1 (mailto:your-real-email@example.com)" `
  -MaxPages 500
```

该脚本具有以下行为：

- 使用 MediaWiki 官方 API。
- 设置 `maxlag=5`。
- 串行发送请求。
- 收到 `429` 或 `503` 时自动退避。
- 只读取主命名空间中的正文页面。
- 只读取分类中的直接页面，不会递归遍历子分类。

需要采集其他主题时，只需更换 `Category`、`OutputFile` 和 `MaxPages`：

```powershell
pwsh -File .\scripts\fetch-wikipedia-category-urls.ps1 `
  -Category "计算机科学" `
  -OutputFile .\data\seed\zhwiki-computer-science.txt `
  -UserAgent "ServerRSKnowledgeBot/0.1 (mailto:your-real-email@example.com)" `
  -MaxPages 500
```

```powershell
pwsh -File .\scripts\fetch-wikipedia-category-urls.ps1 `
  -Category "世界历史" `
  -OutputFile .\data\seed\zhwiki-world-history.txt `
  -UserAgent "ServerRSKnowledgeBot/0.1 (mailto:your-real-email@example.com)" `
  -MaxPages 500
```

## 导入 URL

将生成的 URL 清单导入 MySQL：

```powershell
pwsh -File .\scripts\import-web-source-urls.ps1 `
  -SourceName "zhwiki-artificial-intelligence" `
  -Description "中文维基百科人工智能条目" `
  -UrlFile .\data\seed\zhwiki-artificial-intelligence.txt `
  -AllowedDomains "zh.wikipedia.org" `
  -CrawlIntervalSecs 2592000
```

导入脚本默认使用 `docker-compose.yml` 中的开发环境配置：

```text
容器：serverrs-mysql
数据库：digital_companion
用户：root
密码：passwd
```

需要连接其他数据库时，可传入 `-Container`、`-Database`、`-User` 和
`-Password`。

导入脚本会：

- 只接受 HTTPS URL。
- 校验 URL 是否属于 `AllowedDomains`。
- 创建或更新 `web_sources` 来源。
- 幂等写入 `web_source_urls`，重复运行不会产生重复 URL。
- 默认关闭该来源的自动发布。

确认质量后，可在导入命令中添加 `-AutoPublish`。

每一个主题都应执行一次导入，并使用不同的 `SourceName`。例如：

```powershell
pwsh -File .\scripts\import-web-source-urls.ps1 `
  -SourceName "zhwiki-computer-science" `
  -Description "中文维基百科计算机科学条目" `
  -UrlFile .\data\seed\zhwiki-computer-science.txt `
  -AllowedDomains "zh.wikipedia.org"
```

```powershell
pwsh -File .\scripts\import-web-source-urls.ps1 `
  -SourceName "zhwiki-world-history" `
  -Description "中文维基百科世界历史条目" `
  -UrlFile .\data\seed\zhwiki-world-history.txt `
  -AllowedDomains "zh.wikipedia.org"
```

## 知识质量策略

丰富大模型并不等于无差别收集网页。当前管线实际增强的是 RAG 知识库，
不会修改模型参数。为了让检索结果可靠，建议遵循以下原则：

1. 一个来源对应一个明确主题，不要建立名为 `all-wikipedia` 的超大混合来源。
2. 首轮关闭自动发布，抽查标题、正文、摘要、分块和来源链接。
3. 医学、法律、金融等高风险知识应保持人工审核，不建议自动发布。
4. 优先导入定义明确、结构完整、来源稳定的条目。
5. 排除消歧义页、列表页、年份页、人物争议内容和正文过短的页面。
6. 定期检查重复文档、低质量分块和失效链接。
7. 保留语言与主题边界，中文和英文来源建议分开管理。

推荐的扩展顺序：

1. 计算机、数学、自然科学等结构化程度较高的知识。
2. 历史、地理、经济、哲学等通识知识。
3. 文学、艺术和文化知识。
4. 医学、法律、金融等需要更严格审核的专业知识。

## 一次性创建全部主题

使用批量编排脚本，可以一次创建四组共 25 个主题来源：

```powershell
pwsh -File .\scripts\create-wikipedia-knowledge-tasks.ps1 `
  -UserAgent "ServerRSKnowledgeBot/0.1 (mailto:your-real-email@example.com)"
```

默认使用 `http://127.0.0.1:7890` 代理。需要直连时传入空值：

```powershell
pwsh -File .\scripts\create-wikipedia-knowledge-tasks.ps1 `
  -UserAgent "ServerRSKnowledgeBot/0.1 (mailto:your-real-email@example.com)" `
  -ProxyUrl ""
```

默认行为：

- 每个主题最多获取 300 篇正文。
- 向下遍历一层子分类。
- 在本次批处理中跨主题去除重复 URL。
- 为每个主题创建独立的 `web_sources`。
- 将 URL 幂等导入 `web_source_urls`。
- 所有主题默认关闭自动发布。
- 医学、法学、金融、药学始终要求严格人工审核。
- 在 `data/seed/wikipedia/task-manifest.csv` 输出执行结果。

先预览计划，不访问网络和数据库：

```powershell
pwsh -File .\scripts\create-wikipedia-knowledge-tasks.ps1 `
  -UserAgent "ServerRSKnowledgeBot/0.1 (mailto:your-real-email@example.com)" `
  -PlanOnly
```

只创建指定知识组：

```powershell
# 结构化知识
pwsh -File .\scripts\create-wikipedia-knowledge-tasks.ps1 `
  -UserAgent "ServerRSKnowledgeBot/0.1 (mailto:your-real-email@example.com)" `
  -Groups Structured

# 通识、文学艺术与文化
pwsh -File .\scripts\create-wikipedia-knowledge-tasks.ps1 `
  -UserAgent "ServerRSKnowledgeBot/0.1 (mailto:your-real-email@example.com)" `
  -Groups General,Culture

# 医学、法律、金融、药学
pwsh -File .\scripts\create-wikipedia-knowledge-tasks.ps1 `
  -UserAgent "ServerRSKnowledgeBot/0.1 (mailto:your-real-email@example.com)" `
  -Groups Professional
```

调整每个主题的文章数量和分类深度：

```powershell
pwsh -File .\scripts\create-wikipedia-knowledge-tasks.ps1 `
  -UserAgent "ServerRSKnowledgeBot/0.1 (mailto:your-real-email@example.com)" `
  -MaxPagesPerTopic 500 `
  -CategoryDepth 1
```

`CategoryDepth` 越大，请求数量和主题重叠越多。首次运行建议保持 `1`。
脚本只创建来源和待处理 URL，不会启动服务器；执行完成后运行 `cargo run`
即可触发第一次调度。

如果希望结构化、通识和文化主题通过质量门后自动进入 RAG，增加
`-AutoPublishReviewedGroups`。医学、法学、金融、药学仍保持人工审核：

```powershell
pwsh -File .\scripts\create-wikipedia-knowledge-tasks.ps1 `
  -UserAgent "ServerRSKnowledgeBot/0.1 (mailto:your-real-email@example.com)" `
  -AutoPublishReviewedGroups
```

## 发布人工审核内容

专业知识和未通过自动发布门槛的内容会停留在暂存状态。审核完成后，明确指定
`knowledge_publish_records.id` 创建发布事件：

```powershell
pwsh -File .\scripts\publish-reviewed-web-knowledge.ps1 `
  -PublishRecordIds 101,102,108
```

该脚本不会直接修改知识文档或 Qdrant，而是创建幂等的
`KnowledgePublishRequested` 事件，由服务器完成事务发布和向量激活。

查询待审核记录：

```sql
SELECT pr.id AS publish_record_id,
       kd.title,
       kr.quality_score,
       kr.quality_result,
       kr.risk_flags,
       kd.metadata
FROM knowledge_publish_records pr
JOIN knowledge_ingestion_runs kr ON kr.id = pr.run_id
JOIN knowledge_documents kd ON kd.document_id = pr.document_id
WHERE pr.publish_status = 'staged'
  AND pr.active = 0
ORDER BY pr.created_at DESC;
```

## 启动任务

当前系统没有单独的“启动采集任务”HTTP 接口。以下开关启用后，服务器启动时
会自动启动调度器和事件分发器：

```toml
[web_ingestion]
enabled = true
scheduler_enabled = true
dispatcher_enabled = true
```

在你的命令行中启动服务器：

```powershell
cargo run
```

服务器启动后：

1. 调度器会立即执行第一次 tick。
2. 每个已启用来源会创建一个抓取批次。
3. 每个批次最多投递 `max_urls_per_source_per_job` 个到期 URL。
4. 后续调度间隔由 `scheduler_interval_secs` 控制。

不需要再执行其他命令来启动采集任务。

## 监控任务

查看 outbox 中各类事件的状态：

```sql
SELECT event_type, status, COUNT(*) AS total
FROM domain_event_outbox
GROUP BY event_type, status
ORDER BY event_type, status;
```

查看抓取批次状态：

```sql
SELECT status, COUNT(*) AS total
FROM web_crawl_jobs
GROUP BY status;
```

查看每个来源的 URL 总量和已抓取数量：

```sql
SELECT source_id,
       COUNT(*) AS total,
       SUM(last_crawled_at IS NOT NULL) AS crawled
FROM web_source_urls
GROUP BY source_id;
```

查看最近失败的事件：

```sql
SELECT id, event_type, retry_count, next_retry_at, last_error, updated_at
FROM domain_event_outbox
WHERE status IN ('failed', 'dead')
ORDER BY updated_at DESC
LIMIT 50;
```

## 遇到限流时

如果日志出现 `429` 或 `503`：

1. 不要提高并发或增加采集进程。
2. 增大 `min_request_interval_ms`，例如调整为 `5000`。
3. 保持指数退避开启。
4. 遵守响应中的 `Retry-After`。
5. 检查机器人 UA 是否包含有效联系方式。

不要通过增加代理、轮换身份或伪装浏览器来规避限制。

## 大规模导入

当规模达到数万篇维基百科文章时，应使用 Wikimedia 官方 XML 数据转储，
离线解析后将清洗结果送入知识处理管线。

相比逐页在线抓取，数据转储具有以下优势：

- 对维基百科服务器压力更小。
- 导入速度更快。
- 数据版本固定，结果可复现。
- 更容易暂停、恢复和重新处理。

在线采集器更适合：

- 几百到几千篇经过筛选的条目。
- 小规模专题知识库。
- 已有知识的定期增量更新。
