<#
.SYNOPSIS
为已经人工审核的 Web 知识记录创建发布事件。

.DESCRIPTION
只接受明确指定的 knowledge_publish_records.id。脚本不会直接修改文档
状态，而是写入幂等的 KnowledgePublishRequested outbox 事件，由 ServerRS
执行正常的事务发布、Qdrant 激活和审计流程。
#>

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string[]]$PublishRecordIds,

    [string]$Container = "serverrs-mysql",
    [string]$Database = "digital_companion",
    [string]$User = "root",
    [string]$Password = "passwd",
    [string]$DatabaseUrl = $env:DATABASE_URL,
    [string]$MySqlCommand = "mysql"
)

$ErrorActionPreference = "Stop"

function Get-UrlQueryValue([string]$Query, [string]$Name) {
    if (-not $Query) {
        return $null
    }
    $trimmed = $Query.TrimStart("?")
    foreach ($pair in $trimmed.Split("&", [System.StringSplitOptions]::RemoveEmptyEntries)) {
        $parts = $pair.Split("=", 2)
        $key = [Uri]::UnescapeDataString($parts[0])
        if ($key -eq $Name) {
            if ($parts.Count -eq 1) {
                return ""
            }
            return [Uri]::UnescapeDataString($parts[1])
        }
    }
    return $null
}

function ConvertFrom-DatabaseUrl([string]$Url) {
    $uri = $null
    if (-not [Uri]::TryCreate($Url.Trim(), [UriKind]::Absolute, [ref]$uri)) {
        throw "Invalid DatabaseUrl: $Url"
    }
    if ($uri.Scheme -notin @("mysql", "mariadb")) {
        throw "DatabaseUrl must use mysql:// or mariadb://, got '$($uri.Scheme)://'."
    }

    $userInfo = $uri.UserInfo.Split(":", 2)
    if ($userInfo.Count -eq 0 -or -not $userInfo[0]) {
        throw "DatabaseUrl must include a username."
    }

    $databaseName = [Uri]::UnescapeDataString($uri.AbsolutePath.TrimStart("/"))
    if (-not $databaseName) {
        throw "DatabaseUrl must include a database name path, for example mysql://user:pass@host:3306/db_name."
    }

    [pscustomobject]@{
        Host = $uri.Host
        Port = if ($uri.Port -gt 0) { $uri.Port } else { 3306 }
        User = [Uri]::UnescapeDataString($userInfo[0])
        Password = if ($userInfo.Count -gt 1) { [Uri]::UnescapeDataString($userInfo[1]) } else { "" }
        Database = $databaseName
        SslMode = Get-UrlQueryValue $uri.Query "ssl-mode"
    }
}

function Invoke-MySql([string]$Sql, [switch]$Table) {
    if ($DatabaseUrl -and $DatabaseUrl.Trim()) {
        if (-not (Get-Command $MySqlCommand -ErrorAction SilentlyContinue)) {
            throw "mysql command '$MySqlCommand' was not found. Install MySQL client or omit -DatabaseUrl to use Docker mode."
        }

        $conn = ConvertFrom-DatabaseUrl $DatabaseUrl
        $previousMysqlPwd = $env:MYSQL_PWD
        try {
            $env:MYSQL_PWD = $conn.Password
            $args = @(
                "--host=$($conn.Host)",
                "--port=$($conn.Port)",
                "--user=$($conn.User)",
                "--database=$($conn.Database)",
                "--protocol=TCP",
                "--default-character-set=utf8mb4"
            )
            if ($Table) {
                $args += "--table"
            }
            if ($conn.SslMode) {
                $args += "--ssl-mode=$($conn.SslMode)"
            }
            $Sql | & $MySqlCommand @args
        }
        finally {
            $env:MYSQL_PWD = $previousMysqlPwd
        }
    }
    else {
        if (-not (Get-Command docker -ErrorAction SilentlyContinue)) {
            throw "docker command was not found. Provide -DatabaseUrl to connect with a local mysql client instead."
        }
        $args = @(
            "exec",
            "-i",
            "-e",
            "MYSQL_PWD=$Password",
            $Container,
            "mysql",
            "--user=$User",
            "--database=$Database",
            "--default-character-set=utf8mb4"
        )
        if ($Table) {
            $args += "--table"
        }
        $Sql | docker @args
    }

    if ($LASTEXITCODE -ne 0) {
        throw "mysql command failed with exit code $LASTEXITCODE"
    }
}

$parsedPublishRecordIds = [System.Collections.Generic.List[UInt64]]::new()
foreach ($rawId in $PublishRecordIds) {
    foreach ($candidate in $rawId.Split(",", [System.StringSplitOptions]::RemoveEmptyEntries)) {
        [UInt64]$publishRecordId = 0
        if (-not [UInt64]::TryParse($candidate.Trim(), [ref]$publishRecordId) -or $publishRecordId -eq 0) {
            throw "Invalid publish record ID: '$candidate'"
        }
        $parsedPublishRecordIds.Add($publishRecordId)
    }
}

if ($parsedPublishRecordIds.Count -eq 0) {
    throw "At least one publish record ID is required."
}

$ids = @($parsedPublishRecordIds | Sort-Object -Unique)
$idList = $ids -join ","
$sql = @"
INSERT INTO domain_event_outbox (
    event_key, event_type, aggregate_type, aggregate_id,
    payload, max_retries, status
)
SELECT
    SHA2(CONCAT(
        'KnowledgePublishRequested|knowledge_publish_record|',
        pr.id, '|', pr.run_id, '|', pr.version_key
    ), 256),
    'KnowledgePublishRequested',
    'knowledge_publish_record',
    pr.id,
    JSON_OBJECT(
        'publish_record_id', pr.id,
        'run_id', pr.run_id,
        'automatic', FALSE,
        'reviewed', TRUE
    ),
    5,
    'pending'
FROM knowledge_publish_records pr
JOIN knowledge_ingestion_runs kr ON kr.id = pr.run_id
WHERE pr.id IN ($idList)
  AND pr.publish_status = 'staged'
  AND pr.active = 0
  AND kr.status = 'staged'
  AND kr.stage = 'staging'
ON DUPLICATE KEY UPDATE updated_at = updated_at;

SELECT pr.id AS publish_record_id,
       pr.publish_status,
       pr.active,
       kr.status AS run_status,
       kr.stage AS run_stage
FROM knowledge_publish_records pr
JOIN knowledge_ingestion_runs kr ON kr.id = pr.run_id
WHERE pr.id IN ($idList)
ORDER BY pr.id;
"@

Invoke-MySql $sql -Table

Write-Host "Publish events were created for eligible reviewed records."
Write-Host "Keep ServerRS running so the dispatcher can process them."
