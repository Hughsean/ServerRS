param(
    [Parameter(Mandatory = $true)]
    [string]$Category,

    [Parameter(Mandatory = $true)]
    [string]$OutputFile,

    [Parameter(Mandatory = $true)]
    [string]$UserAgent,

    [string]$ApiUrl = "https://zh.wikipedia.org/w/api.php",
    [string]$ProxyUrl = "http://127.0.0.1:7890",
    [ValidateRange(1, 100000)]
    [int]$MaxPages = 1000,
    [ValidateRange(0, 4)]
    [int]$MaxDepth = 0,
    [ValidateRange(500, 60000)]
    [int]$DelayMs = 1500
)

$ErrorActionPreference = "Stop"

if (-not $Category.StartsWith("Category:")) {
    $Category = "Category:$Category"
}

$headers = @{
    "User-Agent" = $UserAgent
    "Accept" = "application/json"
}

$script:retryDelaySeconds = 5

function Invoke-WikipediaQuery([hashtable]$Query) {
    while ($true) {
        try {
            $requestArgs = @{
                Uri = $ApiUrl
                Method = "Get"
                Headers = $headers
                Body = $Query
            }
            if ($ProxyUrl) {
                $requestArgs["Proxy"] = $ProxyUrl
            }
            $response = Invoke-RestMethod @requestArgs
            if ($response.error.code -eq "maxlag") {
                Write-Warning "Wikipedia reported maxlag; waiting $script:retryDelaySeconds seconds."
                Start-Sleep -Seconds $script:retryDelaySeconds
                $script:retryDelaySeconds = [Math]::Min($script:retryDelaySeconds * 2, 300)
                continue
            }

            $script:retryDelaySeconds = 5
            Start-Sleep -Milliseconds $DelayMs
            return $response
        }
        catch {
            $statusCode = 0
            if ($_.Exception.Response -and $_.Exception.Response.StatusCode) {
                $statusCode = [int]$_.Exception.Response.StatusCode
            }
            if ($statusCode -notin @(429, 503)) {
                throw
            }

            $retryAfter = $null
            $retryHeader = $_.Exception.Response.Headers.RetryAfter
            if ($retryHeader -and $retryHeader.Delta) {
                $retryAfter = [Math]::Ceiling($retryHeader.Delta.TotalSeconds)
            }
            elseif ($retryHeader -and $retryHeader.Date) {
                $retryAfter = [Math]::Ceiling(
                    ($retryHeader.Date.UtcDateTime - [DateTime]::UtcNow).TotalSeconds
                )
            }
            if (-not $retryAfter -or $retryAfter -lt 1) {
                $retryAfter = $script:retryDelaySeconds
                $script:retryDelaySeconds = [Math]::Min($script:retryDelaySeconds * 2, 300)
            }

            Write-Warning "Wikipedia returned HTTP $statusCode; waiting $retryAfter seconds."
            Start-Sleep -Seconds $retryAfter
        }
    }
}

$urls = [System.Collections.Generic.HashSet[string]]::new(
    [System.StringComparer]::OrdinalIgnoreCase
)
$visitedCategories = [System.Collections.Generic.HashSet[string]]::new(
    [System.StringComparer]::OrdinalIgnoreCase
)
$categoryQueue = [System.Collections.Generic.Queue[object]]::new()
$categoryQueue.Enqueue([pscustomobject]@{
    Title = $Category
    Depth = 0
})

while ($categoryQueue.Count -gt 0 -and $urls.Count -lt $MaxPages) {
    $current = $categoryQueue.Dequeue()
    if (-not $visitedCategories.Add([string]$current.Title)) {
        continue
    }

    Write-Host "Reading $($current.Title) at depth $($current.Depth)..."
    $pageContinuation = $null
    do {
        $query = @{
            action = "query"
            generator = "categorymembers"
            gcmtitle = $current.Title
            gcmtype = "page"
            gcmnamespace = 0
            gcmlimit = [Math]::Min(50, $MaxPages - $urls.Count)
            prop = "info"
            inprop = "url"
            format = "json"
            formatversion = 2
            maxlag = 5
        }
        if ($pageContinuation) {
            $query["gcmcontinue"] = $pageContinuation
        }

        $response = Invoke-WikipediaQuery $query
        foreach ($page in $response.query.pages) {
            if ($page.fullurl -and $urls.Count -lt $MaxPages) {
                [void]$urls.Add([string]$page.fullurl)
            }
        }
        $pageContinuation = [string]$response.continue.gcmcontinue
    } while ($pageContinuation -and $urls.Count -lt $MaxPages)

    if ($current.Depth -ge $MaxDepth -or $urls.Count -ge $MaxPages) {
        continue
    }

    $categoryContinuation = $null
    do {
        $query = @{
            action = "query"
            list = "categorymembers"
            cmtitle = $current.Title
            cmtype = "subcat"
            cmnamespace = 14
            cmlimit = 50
            format = "json"
            formatversion = 2
            maxlag = 5
        }
        if ($categoryContinuation) {
            $query["cmcontinue"] = $categoryContinuation
        }

        $response = Invoke-WikipediaQuery $query
        foreach ($subcategory in $response.query.categorymembers) {
            if (-not $visitedCategories.Contains([string]$subcategory.title)) {
                $categoryQueue.Enqueue([pscustomobject]@{
                    Title = [string]$subcategory.title
                    Depth = $current.Depth + 1
                })
            }
        }
        $categoryContinuation = [string]$response.continue.cmcontinue
    } while ($categoryContinuation)
}

$target = [System.IO.Path]::GetFullPath($OutputFile)
$directory = [System.IO.Path]::GetDirectoryName($target)
if ($directory -and -not (Test-Path -LiteralPath $directory)) {
    New-Item -ItemType Directory -Path $directory -Force | Out-Null
}

$urls |
    Sort-Object |
    Set-Content -LiteralPath $target -Encoding utf8

Write-Host "Wrote $($urls.Count) Wikipedia URLs from $($visitedCategories.Count) categories to $target"
