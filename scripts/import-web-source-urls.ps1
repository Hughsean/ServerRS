param(
    [Parameter(Mandatory = $true)]
    [string]$SourceName,

    [Parameter(Mandatory = $true)]
    [string]$UrlFile,

    [Parameter(Mandatory = $true)]
    [string[]]$AllowedDomains,

    [string]$Description = "",
    [string]$Language = "zh",
    [ValidateRange(3600, 31536000)]
    [int]$CrawlIntervalSecs = 2592000,
    [string]$Container = "serverrs-mysql",
    [string]$Database = "digital_companion",
    [string]$User = "root",
    [string]$Password = "passwd",
    [ValidateRange(1, 2000)]
    [int]$BatchSize = 250,
    [switch]$AutoPublish
)

$ErrorActionPreference = "Stop"

function ConvertTo-SqlString([string]$Value) {
    return "'" + $Value.Replace("\", "\\").Replace("'", "''") + "'"
}

function Invoke-MySql([string]$Sql) {
    $Sql | docker exec -i -e "MYSQL_PWD=$Password" $Container `
        mysql "--user=$User" "--database=$Database" `
        --default-character-set=utf8mb4 --batch --raw
    if ($LASTEXITCODE -ne 0) {
        throw "mysql command failed with exit code $LASTEXITCODE"
    }
}

$sourcePath = [System.IO.Path]::GetFullPath($UrlFile)
if (-not (Test-Path -LiteralPath $sourcePath)) {
    throw "URL file does not exist: $sourcePath"
}

$normalizedDomains = $AllowedDomains |
    ForEach-Object { $_.Trim().TrimStart(".").ToLowerInvariant() } |
    Where-Object { $_ } |
    Sort-Object -Unique

if ($normalizedDomains.Count -eq 0) {
    throw "At least one allowed domain is required."
}

$urls = [System.Collections.Generic.List[string]]::new()
foreach ($line in Get-Content -LiteralPath $sourcePath) {
    $candidate = $line.Trim()
    if (-not $candidate -or $candidate.StartsWith("#")) {
        continue
    }

    $uri = $null
    if (-not [Uri]::TryCreate($candidate, [UriKind]::Absolute, [ref]$uri) -or
        $uri.Scheme -ne "https") {
        throw "Only absolute HTTPS URLs are accepted: $candidate"
    }

    $host = $uri.DnsSafeHost.ToLowerInvariant()
    $allowed = $false
    foreach ($domain in $normalizedDomains) {
        if ($host -eq $domain -or $host.EndsWith(".$domain")) {
            $allowed = $true
            break
        }
    }
    if (-not $allowed) {
        throw "URL host '$host' is outside AllowedDomains: $candidate"
    }

    $urls.Add($uri.AbsoluteUri)
}

$urls = @($urls | Sort-Object -Unique)
if ($urls.Count -eq 0) {
    throw "No URLs found in $sourcePath"
}

$sourceNameSql = ConvertTo-SqlString $SourceName
$descriptionSql = ConvertTo-SqlString $Description
$languageSql = ConvertTo-SqlString $Language
$domainsJsonSql = ConvertTo-SqlString (ConvertTo-Json -InputObject @($normalizedDomains) -Compress)
$autoPublishValue = if ($AutoPublish) { 1 } else { 0 }

$sourceSql = @"
SET @source_id = (
    SELECT id FROM web_sources
    WHERE name = $sourceNameSql AND deleted_at IS NULL
    ORDER BY id LIMIT 1
);
INSERT INTO web_sources (
    name, description, approval_status, trust_level, auto_publish,
    allowed_domains, default_language, enabled
)
SELECT
    $sourceNameSql, $descriptionSql, 'approved', 'trusted', $autoPublishValue,
    CAST($domainsJsonSql AS JSON), $languageSql, 1
WHERE @source_id IS NULL;
SET @source_id = COALESCE(@source_id, LAST_INSERT_ID());
UPDATE web_sources
SET description = $descriptionSql,
    approval_status = 'approved',
    trust_level = 'trusted',
    auto_publish = $autoPublishValue,
    allowed_domains = CAST($domainsJsonSql AS JSON),
    default_language = $languageSql,
    enabled = 1
WHERE id = @source_id;
"@
Invoke-MySql $sourceSql

for ($offset = 0; $offset -lt $urls.Count; $offset += $BatchSize) {
    $end = [Math]::Min($offset + $BatchSize - 1, $urls.Count - 1)
    $values = for ($i = $offset; $i -le $end; $i++) {
        $urlSql = ConvertTo-SqlString $urls[$i]
        "(@source_id, $urlSql, $urlSql, SHA2($urlSql, 256), 1, $CrawlIntervalSecs)"
    }

    $batchSql = @"
SET @source_id = (
    SELECT id FROM web_sources
    WHERE name = $sourceNameSql AND deleted_at IS NULL
    ORDER BY id LIMIT 1
);
INSERT INTO web_source_urls (
    source_id, url, canonical_url, url_hash, enabled, crawl_interval_secs
) VALUES
$($values -join ",`n")
ON DUPLICATE KEY UPDATE
    url = VALUES(url),
    canonical_url = VALUES(canonical_url),
    enabled = 1,
    crawl_interval_secs = VALUES(crawl_interval_secs),
    deleted_at = NULL;
"@
    Invoke-MySql $batchSql
    Write-Progress -Activity "Importing web source URLs" `
        -Status "$($end + 1) / $($urls.Count)" `
        -PercentComplete ((($end + 1) / $urls.Count) * 100)
}

Write-Progress -Activity "Importing web source URLs" -Completed
Write-Host "Imported $($urls.Count) URLs into source '$SourceName'."
Write-Host "The scheduler will create crawl jobs when you start the server."
