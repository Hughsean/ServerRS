<#
.SYNOPSIS
一次性创建多主题中文维基百科知识采集任务。

.DESCRIPTION
按主题调用 fetch-wikipedia-category-urls.ps1 生成 URL，再调用
import-web-source-urls.ps1 创建 web_sources 并导入 web_source_urls。
脚本不会启动 ServerRS；服务器启动后的第一次调度会创建抓取批次。
#>

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$UserAgent,

    [string[]]$Groups = @("All"),

    [ValidateRange(1, 10000)]
    [int]$MaxPagesPerTopic = 300,

    [ValidateRange(0, 4)]
    [int]$CategoryDepth = 1,

    [ValidateRange(500, 60000)]
    [int]$ApiDelayMs = 1500,

    [string]$ProxyUrl = "http://127.0.0.1:7890",

    [string]$OutputDirectory = "data/seed/wikipedia",

    [ValidateRange(3600, 31536000)]
    [int]$CrawlIntervalSecs = 2592000,

    [string]$Container = "serverrs-mysql",
    [string]$Database = "digital_companion",
    [string]$User = "root",
    [string]$Password = "passwd",

    [switch]$AutoPublishReviewedGroups,
    [switch]$StopOnError,
    [switch]$PlanOnly
)

$ErrorActionPreference = "Stop"

$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
$fetchScript = Join-Path $PSScriptRoot "fetch-wikipedia-category-urls.ps1"
$importScript = Join-Path $PSScriptRoot "import-web-source-urls.ps1"
$outputRoot = if ([System.IO.Path]::IsPathRooted($OutputDirectory)) {
    [System.IO.Path]::GetFullPath($OutputDirectory)
}
else {
    [System.IO.Path]::GetFullPath((Join-Path $repoRoot $OutputDirectory))
}

$topics = @(
    # 结构化知识
    [pscustomobject]@{ Group = "Structured"; Category = "计算机科学"; Slug = "computer-science"; Description = "中文维基百科计算机科学条目"; StrictReview = $false },
    [pscustomobject]@{ Group = "Structured"; Category = "人工智能"; Slug = "artificial-intelligence"; Description = "中文维基百科人工智能条目"; StrictReview = $false },
    [pscustomobject]@{ Group = "Structured"; Category = "软件工程"; Slug = "software-engineering"; Description = "中文维基百科软件工程条目"; StrictReview = $false },
    [pscustomobject]@{ Group = "Structured"; Category = "数学"; Slug = "mathematics"; Description = "中文维基百科数学条目"; StrictReview = $false },
    [pscustomobject]@{ Group = "Structured"; Category = "物理学"; Slug = "physics"; Description = "中文维基百科物理学条目"; StrictReview = $false },
    [pscustomobject]@{ Group = "Structured"; Category = "化学"; Slug = "chemistry"; Description = "中文维基百科化学条目"; StrictReview = $false },
    [pscustomobject]@{ Group = "Structured"; Category = "生物学"; Slug = "biology"; Description = "中文维基百科生物学条目"; StrictReview = $false },
    [pscustomobject]@{ Group = "Structured"; Category = "天文学"; Slug = "astronomy"; Description = "中文维基百科天文学条目"; StrictReview = $false },
    [pscustomobject]@{ Group = "Structured"; Category = "地球科学"; Slug = "earth-science"; Description = "中文维基百科地球科学条目"; StrictReview = $false },

    # 通识知识
    [pscustomobject]@{ Group = "General"; Category = "历史"; Slug = "history"; Description = "中文维基百科历史条目"; StrictReview = $false },
    [pscustomobject]@{ Group = "General"; Category = "地理学"; Slug = "geography"; Description = "中文维基百科地理学条目"; StrictReview = $false },
    [pscustomobject]@{ Group = "General"; Category = "经济学"; Slug = "economics"; Description = "中文维基百科经济学条目"; StrictReview = $false },
    [pscustomobject]@{ Group = "General"; Category = "哲学"; Slug = "philosophy"; Description = "中文维基百科哲学条目"; StrictReview = $false },
    [pscustomobject]@{ Group = "General"; Category = "社会学"; Slug = "sociology"; Description = "中文维基百科社会学条目"; StrictReview = $false },
    [pscustomobject]@{ Group = "General"; Category = "政治學"; Slug = "political-science"; Description = "中文维基百科政治学条目"; StrictReview = $false },

    # 文学、艺术和文化
    [pscustomobject]@{ Group = "Culture"; Category = "文学"; Slug = "literature"; Description = "中文维基百科文学条目"; StrictReview = $false },
    [pscustomobject]@{ Group = "Culture"; Category = "艺术"; Slug = "arts"; Description = "中文维基百科艺术条目"; StrictReview = $false },
    [pscustomobject]@{ Group = "Culture"; Category = "文化"; Slug = "culture"; Description = "中文维基百科文化条目"; StrictReview = $false },
    [pscustomobject]@{ Group = "Culture"; Category = "音乐"; Slug = "music"; Description = "中文维基百科音乐条目"; StrictReview = $false },
    [pscustomobject]@{ Group = "Culture"; Category = "建筑学"; Slug = "architecture"; Description = "中文维基百科建筑学条目"; StrictReview = $false },
    [pscustomobject]@{ Group = "Culture"; Category = "宗教"; Slug = "religion"; Description = "中文维基百科宗教条目"; StrictReview = $false },

    # 需要严格审核的专业知识
    [pscustomobject]@{ Group = "Professional"; Category = "医学"; Slug = "medicine"; Description = "中文维基百科医学条目，必须人工审核"; StrictReview = $true },
    [pscustomobject]@{ Group = "Professional"; Category = "法学"; Slug = "law"; Description = "中文维基百科法学条目，必须人工审核"; StrictReview = $true },
    [pscustomobject]@{ Group = "Professional"; Category = "金融"; Slug = "finance"; Description = "中文维基百科金融条目，必须人工审核"; StrictReview = $true },
    [pscustomobject]@{ Group = "Professional"; Category = "藥學"; Slug = "pharmacy"; Description = "中文维基百科药学条目，必须人工审核"; StrictReview = $true }
)

$validGroups = @("All", "Structured", "General", "Culture", "Professional")
$requestedGroups = @(
    $Groups |
        ForEach-Object { $_ -split "," } |
        ForEach-Object { $_.Trim() } |
        Where-Object { $_ } |
        Select-Object -Unique
)
$invalidGroups = @($requestedGroups | Where-Object { $_ -notin $validGroups })
if ($invalidGroups.Count -gt 0) {
    throw "Invalid group(s): $($invalidGroups -join ', '). Valid groups: $($validGroups -join ', ')."
}

$selectedGroups = if ($requestedGroups -contains "All") {
    @("Structured", "General", "Culture", "Professional")
}
else {
    $requestedGroups
}
$selectedTopics = @($topics | Where-Object { $_.Group -in $selectedGroups })

if ($selectedTopics.Count -eq 0) {
    throw "No topics were selected."
}

Write-Host ""
Write-Host "Wikipedia knowledge task plan"
Write-Host "Topics: $($selectedTopics.Count)"
Write-Host "Maximum pages per topic: $MaxPagesPerTopic"
Write-Host "Category depth: $CategoryDepth"
Write-Host "Proxy: $(if ($ProxyUrl) { $ProxyUrl } else { '<direct>' })"
Write-Host "Output directory: $outputRoot"
Write-Host ""

$selectedTopics |
    Select-Object Group, Category, Slug, StrictReview |
    Format-Table -AutoSize

if ($PlanOnly) {
    Write-Host "PlanOnly enabled; no network or database changes were made."
    return
}

if (-not (Test-Path -LiteralPath $fetchScript)) {
    throw "Missing fetch script: $fetchScript"
}
if (-not (Test-Path -LiteralPath $importScript)) {
    throw "Missing import script: $importScript"
}
if (-not (Get-Command docker -ErrorAction SilentlyContinue)) {
    throw "docker command was not found."
}

$containerRunning = docker inspect --format "{{.State.Running}}" $Container 2>$null
if ($LASTEXITCODE -ne 0 -or $containerRunning.Trim() -ne "true") {
    throw "MySQL container '$Container' is not running."
}

$categoryTitles = ($selectedTopics | ForEach-Object { "Category:$($_.Category)" }) -join "|"
$categoryCheckArgs = @{
    Uri = "https://zh.wikipedia.org/w/api.php"
    Method = "Get"
    Headers = @{ "User-Agent" = $UserAgent; "Accept" = "application/json" }
    Body = @{
        action = "query"
        prop = "categoryinfo"
        titles = $categoryTitles
        format = "json"
        formatversion = 2
        maxlag = 5
    }
}
if ($ProxyUrl) {
    $categoryCheckArgs["Proxy"] = $ProxyUrl
}
$categoryCheck = Invoke-RestMethod @categoryCheckArgs
$missingCategories = @(
    $categoryCheck.query.pages |
        Where-Object { $_.missing } |
        ForEach-Object { $_.title }
)
if ($missingCategories.Count -gt 0) {
    throw "Wikipedia categories do not exist: $($missingCategories -join ', ')"
}

New-Item -ItemType Directory -Path $outputRoot -Force | Out-Null

$seenUrls = [System.Collections.Generic.HashSet[string]]::new(
    [System.StringComparer]::OrdinalIgnoreCase
)
$results = [System.Collections.Generic.List[object]]::new()

foreach ($topic in $selectedTopics) {
    $sourceName = "zhwiki-$($topic.Slug)"
    $urlFile = Join-Path $outputRoot "$sourceName.txt"
    $autoPublish = $AutoPublishReviewedGroups -and -not $topic.StrictReview

    Write-Host ""
    Write-Host "[$($topic.Group)] $($topic.Category) -> $sourceName"

    try {
        & $fetchScript `
            -Category $topic.Category `
            -OutputFile $urlFile `
            -UserAgent $UserAgent `
            -MaxPages $MaxPagesPerTopic `
            -MaxDepth $CategoryDepth `
            -ProxyUrl $ProxyUrl `
            -DelayMs $ApiDelayMs

        $topicUrls = [System.Collections.Generic.List[string]]::new()
        foreach ($url in Get-Content -LiteralPath $urlFile) {
            $candidate = $url.Trim()
            if ($candidate -and $seenUrls.Add($candidate)) {
                $topicUrls.Add($candidate)
            }
        }

        $topicUrls |
            Sort-Object |
            Set-Content -LiteralPath $urlFile -Encoding utf8

        if ($topicUrls.Count -eq 0) {
            throw "No unique article URLs were found for category '$($topic.Category)'."
        }

        $importArgs = @{
            SourceName = $sourceName
            Description = $topic.Description
            UrlFile = $urlFile
            AllowedDomains = @("zh.wikipedia.org")
            CrawlIntervalSecs = $CrawlIntervalSecs
            Container = $Container
            Database = $Database
            User = $User
            Password = $Password
        }
        if ($autoPublish) {
            $importArgs["AutoPublish"] = $true
        }
        & $importScript @importArgs

        $results.Add([pscustomobject]@{
            Group = $topic.Group
            Category = $topic.Category
            SourceName = $sourceName
            UrlCount = $topicUrls.Count
            AutoPublish = $autoPublish
            Status = "created"
            Error = ""
        })
    }
    catch {
        $results.Add([pscustomobject]@{
            Group = $topic.Group
            Category = $topic.Category
            SourceName = $sourceName
            UrlCount = 0
            AutoPublish = $false
            Status = "failed"
            Error = $_.Exception.Message
        })
        Write-Warning "Failed to create '$sourceName': $($_.Exception.Message)"
        if ($StopOnError) {
            throw
        }
    }
}

$manifestPath = Join-Path $outputRoot "task-manifest.csv"
$results | Export-Csv -LiteralPath $manifestPath -NoTypeInformation -Encoding utf8

Write-Host ""
Write-Host "Task creation summary"
$results | Format-Table Group, Category, UrlCount, AutoPublish, Status -AutoSize
Write-Host "Manifest: $manifestPath"
Write-Host "Start ServerRS with 'cargo run' to trigger the first scheduler tick."

$failedCount = @($results | Where-Object { $_.Status -eq "failed" }).Count
if ($failedCount -gt 0) {
    throw "$failedCount topic task(s) failed. See $manifestPath."
}
