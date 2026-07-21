<#
.SYNOPSIS
一次性创建多主题中文维基百科知识采集任务。

.DESCRIPTION
按主题调用 2.fetch-urls.ps1 生成 URL，再调用
3.import-urls.ps1 创建 web_sources 并导入 web_source_urls。
传入 -ExportOnly 时只生成 URL 文件和 manifest，不写数据库。
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

    [ValidateRange(50, 60000)]
    [int]$ApiDelayMs = 1500,

    [ValidateRange(1, 16)]
    [int]$Parallelism = 4,

    [string]$ProxyUrl = "http://127.0.0.1:7890",

    [string]$OutputDirectory = "data/seed/wikipedia",

    [ValidateRange(3600, 31536000)]
    [int]$CrawlIntervalSecs = 2592000,

    [string]$Container = "serverrs-mysql",
    [string]$Database = "digital_companion",
    [string]$User = "root",
    [string]$Password = "passwd",
    [string]$DatabaseUrl = $env:DATABASE_URL,
    [string]$MySqlCommand = "mysql",

    [switch]$AutoPublishReviewedGroups,
    [switch]$ExportOnly,
    [switch]$StopOnError,
    [switch]$PlanOnly
)

$ErrorActionPreference = "Stop"

$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot "..\.."))
$fetchScript = Join-Path $PSScriptRoot "2.fetch-urls.ps1"
$importScript = Join-Path $PSScriptRoot "3.import-urls.ps1"
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

    # 更细分的高密度知识
    [pscustomobject]@{ Group = "Specialized"; Category = "密码学"; Slug = "cryptography"; Description = "中文维基百科密码学条目"; StrictReview = $false },
    [pscustomobject]@{ Group = "Specialized"; Category = "信息论"; Slug = "information-theory"; Description = "中文维基百科信息论条目"; StrictReview = $false },
    [pscustomobject]@{ Group = "Specialized"; Category = "控制论"; Slug = "cybernetics"; Description = "中文维基百科控制论条目"; StrictReview = $false },
    [pscustomobject]@{ Group = "Specialized"; Category = "博弈论"; Slug = "game-theory"; Description = "中文维基百科博弈论条目"; StrictReview = $false },
    [pscustomobject]@{ Group = "Specialized"; Category = "图论"; Slug = "graph-theory"; Description = "中文维基百科图论条目"; StrictReview = $false },
    [pscustomobject]@{ Group = "Specialized"; Category = "拓扑学"; Slug = "topology"; Description = "中文维基百科拓扑学条目"; StrictReview = $false },
    [pscustomobject]@{ Group = "Specialized"; Category = "形式语言"; Slug = "formal-languages"; Description = "中文维基百科形式语言条目"; StrictReview = $false },
    [pscustomobject]@{ Group = "Specialized"; Category = "编译原理"; Slug = "compiler-theory"; Description = "中文维基百科编译原理条目"; StrictReview = $false },
    [pscustomobject]@{ Group = "Specialized"; Category = "数据库理论"; Slug = "database-theory"; Description = "中文维基百科数据库理论条目"; StrictReview = $false },
    [pscustomobject]@{ Group = "Specialized"; Category = "分布式计算"; Slug = "distributed-computing"; Description = "中文维基百科分布式计算条目"; StrictReview = $false },
    [pscustomobject]@{ Group = "Specialized"; Category = "计算语言学"; Slug = "computational-linguistics"; Description = "中文维基百科计算语言学条目"; StrictReview = $false },
    [pscustomobject]@{ Group = "Specialized"; Category = "人类学"; Slug = "anthropology"; Description = "中文维基百科人类学条目"; StrictReview = $false },
    [pscustomobject]@{ Group = "Specialized"; Category = "考古学"; Slug = "archaeology"; Description = "中文维基百科考古学条目"; StrictReview = $false },
    [pscustomobject]@{ Group = "Specialized"; Category = "文献学"; Slug = "philology"; Description = "中文维基百科文献学条目"; StrictReview = $false },
    [pscustomobject]@{ Group = "Specialized"; Category = "语言学"; Slug = "linguistics"; Description = "中文维基百科语言学条目"; StrictReview = $false },
    [pscustomobject]@{ Group = "Specialized"; Category = "文字"; Slug = "writing-systems"; Description = "中文维基百科文字条目"; StrictReview = $false },
    [pscustomobject]@{ Group = "Specialized"; Category = "美学"; Slug = "aesthetics"; Description = "中文维基百科美学条目"; StrictReview = $false },
    [pscustomobject]@{ Group = "Specialized"; Category = "气象学"; Slug = "meteorology"; Description = "中文维基百科气象学条目"; StrictReview = $false },
    [pscustomobject]@{ Group = "Specialized"; Category = "海洋学"; Slug = "oceanography"; Description = "中文维基百科海洋学条目"; StrictReview = $false },
    [pscustomobject]@{ Group = "Specialized"; Category = "宇宙学"; Slug = "cosmology"; Description = "中文维基百科宇宙学条目"; StrictReview = $false },
    [pscustomobject]@{ Group = "Specialized"; Category = "机械工程"; Slug = "mechanical-engineering"; Description = "中文维基百科机械工程条目"; StrictReview = $false },
    [pscustomobject]@{ Group = "Specialized"; Category = "电子工程"; Slug = "electronic-engineering"; Description = "中文维基百科电子工程条目"; StrictReview = $false },
    [pscustomobject]@{ Group = "Specialized"; Category = "通信工程"; Slug = "telecommunications-engineering"; Description = "中文维基百科通信工程条目"; StrictReview = $false },
    [pscustomobject]@{ Group = "Specialized"; Category = "声学"; Slug = "acoustics"; Description = "中文维基百科声学条目"; StrictReview = $false },
    [pscustomobject]@{ Group = "Specialized"; Category = "流体力学"; Slug = "fluid-dynamics"; Description = "中文维基百科流体力学条目"; StrictReview = $false },
    [pscustomobject]@{ Group = "Specialized"; Category = "城市规划"; Slug = "urban-planning"; Description = "中文维基百科城市规划条目"; StrictReview = $false },

    # 需要严格审核的专业知识
    [pscustomobject]@{ Group = "Professional"; Category = "医学"; Slug = "medicine"; Description = "中文维基百科医学条目，必须人工审核"; StrictReview = $true },
    [pscustomobject]@{ Group = "Professional"; Category = "法学"; Slug = "law"; Description = "中文维基百科法学条目，必须人工审核"; StrictReview = $true },
    [pscustomobject]@{ Group = "Professional"; Category = "金融"; Slug = "finance"; Description = "中文维基百科金融条目，必须人工审核"; StrictReview = $true },
    [pscustomobject]@{ Group = "Professional"; Category = "藥學"; Slug = "pharmacy"; Description = "中文维基百科药学条目，必须人工审核"; StrictReview = $true }
)

$validGroups = @("All", "Structured", "General", "Culture", "Specialized", "Professional")
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
    @("Structured", "General", "Culture", "Specialized", "Professional")
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
Write-Host "Parallelism: $Parallelism"
Write-Host "Proxy: $(if ($ProxyUrl) { $ProxyUrl } else { '<direct>' })"
Write-Host "Output directory: $outputRoot"
Write-Host "Mode: $(if ($ExportOnly) { 'export URL files only' } else { 'export URL files and import database sources' })"
if (-not $ExportOnly) {
    Write-Host "Database: $(if ($DatabaseUrl) { 'direct URL connection' } else { "docker container '$Container'" })"
}
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
if (-not (Get-Command Start-ThreadJob -ErrorAction SilentlyContinue)) {
    throw "Start-ThreadJob was not found. Run with PowerShell 7+ or install the ThreadJob module."
}

if (-not $ExportOnly) {
    if (-not (Test-Path -LiteralPath $importScript)) {
        throw "Missing import script: $importScript"
    }

    if ($DatabaseUrl -and $DatabaseUrl.Trim()) {
        if (-not (Get-Command $MySqlCommand -ErrorAction SilentlyContinue)) {
            throw "mysql command '$MySqlCommand' was not found. Install MySQL client or omit -DatabaseUrl to use Docker mode."
        }
    }
    else {
        if (-not (Get-Command docker -ErrorAction SilentlyContinue)) {
            throw "docker command was not found. Provide -DatabaseUrl to connect with a local mysql client instead."
        }

        $containerRunning = docker inspect --format "{{.State.Running}}" $Container 2>$null
        if ($LASTEXITCODE -ne 0 -or $containerRunning.Trim() -ne "true") {
            throw "MySQL container '$Container' is not running."
        }
    }
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

$jobs = [System.Collections.Generic.List[object]]::new()
foreach ($topic in $selectedTopics) {
    $sourceName = "zhwiki-$($topic.Slug)"
    while (@($jobs | Where-Object { $_.State -eq "Running" }).Count -ge $Parallelism) {
        $completed = Wait-Job -Job $jobs -Any
        foreach ($job in @($completed)) {
            Write-Host "Fetched topic job finished: $($job.Name)"
        }
    }

    Write-Host "Queueing [$($topic.Group)] $($topic.Category) -> $sourceName"
    $jobs.Add((Start-ThreadJob -Name $sourceName -ScriptBlock {
        param(
            [pscustomobject]$Topic,
            [string]$FetchScript,
            [string]$OutputRoot,
            [string]$UserAgent,
            [int]$MaxPagesPerTopic,
            [int]$CategoryDepth,
            [string]$ProxyUrl,
            [int]$ApiDelayMs,
            [bool]$AutoPublishReviewedGroups
        )

        $ErrorActionPreference = "Stop"
        $sourceName = "zhwiki-$($Topic.Slug)"
        $urlFile = Join-Path $OutputRoot "$sourceName.txt"
        $autoPublish = $AutoPublishReviewedGroups -and -not [bool]$Topic.StrictReview

        try {
            & $FetchScript `
                -Category $Topic.Category `
                -OutputFile $urlFile `
                -UserAgent $UserAgent `
                -MaxPages $MaxPagesPerTopic `
                -MaxDepth $CategoryDepth `
                -ProxyUrl $ProxyUrl `
                -DelayMs $ApiDelayMs `
                -Quiet

            $rawUrls = @(
                Get-Content -LiteralPath $urlFile |
                    ForEach-Object { $_.Trim() } |
                    Where-Object { $_ }
            )

            [pscustomobject]@{
                Group = $Topic.Group
                Category = $Topic.Category
                Description = $Topic.Description
                SourceName = $sourceName
                UrlFile = $urlFile
                RawUrls = $rawUrls
                AutoPublish = $autoPublish
                Status = "fetched"
                Error = ""
            }
        }
        catch {
            [pscustomobject]@{
                Group = $Topic.Group
                Category = $Topic.Category
                Description = $Topic.Description
                SourceName = $sourceName
                UrlFile = $urlFile
                RawUrls = @()
                AutoPublish = $autoPublish
                Status = "failed"
                Error = $_.Exception.Message
            }
        }
    } -ArgumentList @(
        $topic,
        $fetchScript,
        $outputRoot,
        $UserAgent,
        $MaxPagesPerTopic,
        $CategoryDepth,
        $ProxyUrl,
        $ApiDelayMs,
        [bool]$AutoPublishReviewedGroups
    )))
}

if ($jobs.Count -gt 0) {
    Wait-Job -Job $jobs | Out-Null
}

$fetchResults = @(
    foreach ($job in $jobs) {
        Receive-Job -Job $job
    }
)
if ($jobs.Count -gt 0) {
    Remove-Job -Job $jobs
}

$fetchBySource = @{}
foreach ($fetchResult in $fetchResults) {
    $fetchBySource[$fetchResult.SourceName] = $fetchResult
}

$seenUrls = [System.Collections.Generic.HashSet[string]]::new(
    [System.StringComparer]::OrdinalIgnoreCase
)
$results = [System.Collections.Generic.List[object]]::new()

foreach ($topic in $selectedTopics) {
    $sourceName = "zhwiki-$($topic.Slug)"
    $fetchResult = $fetchBySource[$sourceName]
    if ($null -eq $fetchResult) {
        $results.Add([pscustomobject]@{
            Group = $topic.Group
            Category = $topic.Category
            SourceName = $sourceName
            UrlCount = 0
            AutoPublish = $false
            Status = "failed"
            Error = "fetch job produced no result"
        })
        if ($StopOnError) {
            break
        }
        continue
    }

    if ($fetchResult.Status -eq "failed") {
        $results.Add([pscustomobject]@{
            Group = $fetchResult.Group
            Category = $fetchResult.Category
            SourceName = $fetchResult.SourceName
            UrlCount = 0
            AutoPublish = $fetchResult.AutoPublish
            Status = "failed"
            Error = $fetchResult.Error
        })
        Write-Warning "Failed to fetch '$($fetchResult.SourceName)': $($fetchResult.Error)"
        if ($StopOnError) {
            break
        }
        continue
    }

    $topicUrls = [System.Collections.Generic.List[string]]::new()
    foreach ($url in @($fetchResult.RawUrls)) {
        $candidate = $url.Trim()
        if ($candidate -and $seenUrls.Add($candidate)) {
            $topicUrls.Add($candidate)
        }
    }

    $topicUrls |
        Sort-Object |
        Set-Content -LiteralPath $fetchResult.UrlFile -Encoding utf8

    if ($topicUrls.Count -eq 0) {
        $results.Add([pscustomobject]@{
            Group = $fetchResult.Group
            Category = $fetchResult.Category
            SourceName = $fetchResult.SourceName
            UrlCount = 0
            AutoPublish = $fetchResult.AutoPublish
            Status = "failed"
            Error = "No unique article URLs were found for category '$($fetchResult.Category)'."
        })
        if ($StopOnError) {
            break
        }
        continue
    }

    if ($ExportOnly) {
        $results.Add([pscustomobject]@{
            Group = $fetchResult.Group
            Category = $fetchResult.Category
            SourceName = $fetchResult.SourceName
            UrlCount = $topicUrls.Count
            AutoPublish = $fetchResult.AutoPublish
            Status = "exported"
            Error = ""
        })
        continue
    }

    try {
        $importArgs = @{
            SourceName = $fetchResult.SourceName
            Description = $fetchResult.Description
            UrlFile = $fetchResult.UrlFile
            AllowedDomains = @("zh.wikipedia.org")
            CrawlIntervalSecs = $CrawlIntervalSecs
            Container = $Container
            Database = $Database
            User = $User
            Password = $Password
            MySqlCommand = $MySqlCommand
        }
        if ($DatabaseUrl -and $DatabaseUrl.Trim()) {
            $importArgs["DatabaseUrl"] = $DatabaseUrl
        }
        if ($fetchResult.AutoPublish) {
            $importArgs["AutoPublish"] = $true
        }
        & $importScript @importArgs

        $results.Add([pscustomobject]@{
            Group = $fetchResult.Group
            Category = $fetchResult.Category
            SourceName = $fetchResult.SourceName
            UrlCount = $topicUrls.Count
            AutoPublish = $fetchResult.AutoPublish
            Status = "created"
            Error = ""
        })
    }
    catch {
        $results.Add([pscustomobject]@{
            Group = $fetchResult.Group
            Category = $fetchResult.Category
            SourceName = $fetchResult.SourceName
            UrlCount = $topicUrls.Count
            AutoPublish = $fetchResult.AutoPublish
            Status = "failed"
            Error = $_.Exception.Message
        })
        Write-Warning "Failed to import '$($fetchResult.SourceName)': $($_.Exception.Message)"
        if ($StopOnError) {
            break
        }
    }
}

$manifestPath = Join-Path $outputRoot "task-manifest.csv"
$results | Export-Csv -LiteralPath $manifestPath -NoTypeInformation -Encoding utf8

Write-Host ""
Write-Host "$(if ($ExportOnly) { 'URL export summary' } else { 'Task creation summary' })"
$results | Format-Table Group, Category, UrlCount, AutoPublish, Status -AutoSize
Write-Host "Manifest: $manifestPath"
if ($ExportOnly) {
    Write-Host "URL files were exported only; run this script without -ExportOnly or import-web-source-urls.ps1 to load them into web ingestion."
}
else {
    Write-Host "Start ServerRS with 'cargo run -p server --bin server-rs' to trigger the first scheduler tick."
}

$failedCount = @($results | Where-Object { $_.Status -eq "failed" }).Count
if ($failedCount -gt 0) {
    throw "$failedCount topic task(s) failed. See $manifestPath."
}
