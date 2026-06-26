<#
.SYNOPSIS
  真实对话压力测试脚本 — 模拟 alice 用户进行多轮对话
.DESCRIPTION
  调用运行中的 ServerRS HTTP API，模拟真实用户连续对话。
  每次发送消息后等待 LLM 回复完成，再发下一条。
  覆盖个人信息、日常生活、情绪感受、兴趣爱好四大类话题。
.NOTES
  需先启动服务器 (cargo rr --bin server-rs)
  依赖 PowerShell 7+ (pwsh)
#>

param(
    [string]$BaseUrl = "http://127.0.0.1:8080",
    [string]$Username = "test",
    [string]$Password = "123123123",
    [int]$MessageCount = 30,
    [switch]$ShowReply
)

$ProgressPreference = 'SilentlyContinue'
$ErrorActionPreference = 'Stop'

# ── 工具函数 ────────────────────────────────────────────────────────

function Write-Step($msg) {
    $time = Get-Date -Format "HH:mm:ss"
    Write-Host "[$time] $msg"
}

function Invoke-Api {
    param($Method, $Uri, $Body, $Token)
    $headers = @{ "Content-Type" = "application/json" }
    if ($Token) { $headers["Authorization"] = "Bearer $Token" }
    $params = @{
        Method = $Method
        Uri = $Uri
        Headers = $headers
    }
    if ($Body) { $params["Body"] = ($Body | ConvertTo-Json -Compress) }
    try {
        $resp = Invoke-RestMethod @params
        return $resp
    } catch {
        $statusCode = $_.Exception.Response.StatusCode.value__
        $detail = try { $_ | ConvertFrom-Json -ErrorAction Stop } catch { $_.Exception.Message }
        Write-Host "  ⛔ API 错误 [$statusCode]: $detail" -ForegroundColor Red
        throw
    }
}

# ── 对话剧本（话题多样，覆盖记忆触发点） ──────────────────────────

$script:messages = @(
    # ── 个人信息 ──
    "你好，我叫 Alice，今年 24 岁，是一名平面设计师。很高兴认识你！",
    "我平时喜欢画画和摄影，周末经常去公园拍风景。",
    "我养了一只橘猫叫小橘，它特别黏人，每天都要趴在我腿上睡觉。",
    "我在上海工作，公司在静安区，通勤大概要一个小时。",
    "我老家在成都，特别想念家乡的火锅和串串香。",

    # ── 日常生活 ──
    "今天加班到晚上九点，好累啊，感觉眼睛都快睁不开了。",
    "早上起来发现下雨了，出门忘带伞，淋了一路到地铁站。",
    "中午点外卖点了一份酸菜鱼，味道还不错，就是刺有点多。",
    "周末去了趟宜家，买了一个新的书桌，自己组装了两个小时。",
    "最近在学做菜，昨天尝试了番茄炒蛋，虽然卖相一般但味道还行。",

    # ── 情绪感受 ──
    "今天被领导表扬了，说我最近的设计方案很有创意，心情特别好。",
    "有点沮丧，跟男朋友吵架了，因为一些小事，现在不想说话。",
    "最近总是失眠，躺床上一两个小时都睡不着，脑子里乱七八糟的。",
    "今天去健身房跑了一个小时，出了一身汗，感觉整个人都轻松了。",
    "好焦虑，下周要做一个重要的提案，感觉还没准备好。",

    # ── 兴趣爱好 ──
    "最近在看一部日剧《重启人生》，剧情很治愈，推荐给你。",
    "想学吉他，小时候学过两年后来放弃了，现在想捡起来。",
    "周末去看了个摄影展，有一组黑白街拍特别有感觉，给了我很多灵感。",
    "买了几本新书，东野圭吾的最新推理小说，周末窝在家看书。",
    "最近迷上了做手账，买了好多贴纸和胶带，感觉好解压。",

    # ── 深度话题 ──
    "我在想要不要辞职去读研，感觉现在的工作遇到了瓶颈期。",
    "和妈妈视频电话聊了一个小时，她说想我了，有点想回家。",
    "今天在地铁上看到一个老奶奶提不动东西，帮了她一把，她说谢谢的时候我好开心。",
    "觉得孤独，虽然在上海有好朋友，但有时候还是会觉得一个人。",
    "我想养第二只猫，给橘猫找个伴，但又怕它们打架。",

    # ── 更多日常 ──
    "今天刷到一条流浪狗的视频，看哭了，太可怜了。",
    "同事推荐了一款新的咖啡豆，买回来试了一下，确实比速溶好喝太多了。",
    "最近在学英语，准备考雅思，目标是 7 分。",
    "双十一买了一堆东西，快递堆了一客厅，拆快递拆到手软。",
    "今天下班路上看到夕阳特别美，停下来拍了张照片，治愈了一天的疲惫。"
)

# ── 主流程 ──────────────────────────────────────────────────────────

Write-Host "══════════════════════════════════════════" -ForegroundColor Cyan
Write-Host " ServerRS 实机对话测试脚本" -ForegroundColor Cyan
Write-Host " 用户: $Username  |  消息数: $MessageCount" -ForegroundColor Cyan
Write-Host " 地址: $BaseUrl" -ForegroundColor Cyan
Write-Host "══════════════════════════════════════════" -ForegroundColor Cyan
Write-Host ""

# 1. 登录
Write-Step "🔑 登录中..."
$loginResp = Invoke-Api -Method Post -Uri "$BaseUrl/api/v1/auth/login" -Body @{
    username = $Username
    password = $Password
}
$token = $loginResp.accessToken
$userId = $loginResp.user.id
Write-Step "✅ 登录成功  user_id=$userId  token=${token.Substring(0,20)}..."

# 2. 打开/获取会话
Write-Step "💬 打开会话..."
$chatResp = Invoke-Api -Method Post -Uri "$BaseUrl/api/v1/chat/open" -Token $token
$conversationId = $chatResp.conversation.id
Write-Step "✅ 会话已就绪  conversation_id=$conversationId"

# 3. 轮询发送消息
Write-Host ""
Write-Host "────────── 开始发送 $MessageCount 条消息 ──────────" -ForegroundColor Cyan

$totalDuration = [Diagnostics.Stopwatch]::StartNew()
$successCount = 0
$failCount = 0

for ($i = 0; $i -lt $MessageCount; $i++) {
    $text = $script:messages[$i % $script:messages.Count]
    $round = $i + 1
    $emoji = @("😊", "😌", "😄", "🤔", "🥰", "😅", "😢", "🎨", "📸", "🐱")[$i % 10]

    Write-Step "[$round/$MessageCount] 发送: $($text.Substring(0, [Math]::Min(30, $text.Length)))..."

    $sw = [Diagnostics.Stopwatch]::StartNew()
    try {
        $resp = Invoke-Api -Method Post -Uri "$BaseUrl/api/v1/chat/messages" `
            -Token $token -Body @{ text = "$emoji $text" }
        $sw.Stop()

        if ($ShowReply) {
            Write-Host "     🤖 $($resp.reply.Substring(0, [Math]::Min(80, $resp.reply.Length)))" -ForegroundColor DarkYellow
        }

        $toolCount = if ($resp.toolCalls) { $resp.toolCalls.Count } else { 0 }
        $replyLen = $resp.reply.Length
        Write-Host "     ✅ $([Math]::Round($sw.Elapsed.TotalSeconds, 1))s  |  回复 ${replyLen}字  |  工具调用 ${toolCount}次" -ForegroundColor Green
        $successCount++

    } catch {
        $sw.Stop()
        Write-Host "     ❌ $([Math]::Round($sw.Elapsed.TotalSeconds, 1))s 失败" -ForegroundColor Red
        $failCount++
    }

    # 消息间隔 0.5 秒，避免压垮 Ollama
    Start-Sleep -Milliseconds 500
}

$totalDuration.Stop()

# ── 汇总 ────────────────────────────────────────────────────────────

Write-Host ""
Write-Host "══════════════════════════════════════════" -ForegroundColor Cyan
Write-Host " 📊 测试完成" -ForegroundColor Cyan
Write-Host "══════════════════════════════════════════" -ForegroundColor Cyan
Write-Host " 总用时:       $([Math]::Round($totalDuration.Elapsed.TotalSeconds, 1))s"
Write-Host " 成功:         $successCount"
Write-Host " 失败:         $failCount"
Write-Host " 平均耗时:     $([Math]::Round(($totalDuration.Elapsed.TotalSeconds) / [Math]::Max(1, $successCount), 1))s/条"
Write-Host ""

# 4. 查看到底提取了多少记忆
Write-Step "📦 查看到底提取了多少记忆..."
Start-Sleep -Seconds 2
$memoriesResp = Invoke-Api -Method Get -Uri "$BaseUrl/api/v1/chat/memories" -Token $token
Write-Host "  🧠 活跃记忆: $($memoriesResp.totalActive) 条" -ForegroundColor Yellow
if ($memoriesResp.memories) {
    $memoriesResp.memories | ForEach-Object {
        Write-Host "     · [$($_.memoryType)] $($_.content.Substring(0, [Math]::Min(50, $_.content.Length)))" -ForegroundColor DarkYellow
    }
}

# 5. 查看画像
Write-Step "👤 查看用户画像..."
Start-Sleep -Seconds 1
try {
    $personaResp = Invoke-Api -Method Get -Uri "$BaseUrl/api/v1/chat/persona" -Token $token
    Write-Host "  🆔 活跃画像: $($personaResp.hasActivePersona)" -ForegroundColor Yellow
    Write-Host "     交流偏好: $($personaResp.snapshotSummary.communicationPreferencesCount)" -ForegroundColor Yellow
    Write-Host "     稳定事实: $($personaResp.snapshotSummary.stableFactsCount)" -ForegroundColor Yellow
    Write-Host "     重复话题: $($personaResp.snapshotSummary.recurringTopicsCount)" -ForegroundColor Yellow
    Write-Host "     个人目标: $($personaResp.snapshotSummary.goalsCount)" -ForegroundColor Yellow
} catch {
    Write-Host "  ⛔ 获取画像失败: $_" -ForegroundColor Red
}

Write-Host ""
Write-Host "✅ 测试完成！" -ForegroundColor Green
