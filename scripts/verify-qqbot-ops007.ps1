<#
.SYNOPSIS
  QQBot 独立数据库与 Spool 密钥轮换的安全演练。

.DESCRIPTION
  仅允许使用本地 `serverrs-qqbot-mysql` 容器，并只创建随机
  `qqbot_accept_ops007_*` schema。演练覆盖：
  1. Baseline + 增量迁移加载；
  2. 单事务 mysqldump 备份与异名 schema 恢复；
  3. 单账号 JSONL 数据导出；
  4. 单账号彻底删除及显式账号引用/级联残留扫描；
  5. Recall/Realtime Spool 空 backlog 密钥轮换 fail-closed 测试。

  不读取 QQBot 生产数据库 URL，不接触数字人容器/数据库，不连接 QQ/NapCat。
#>

[CmdletBinding()]
param(
    [string]$Container = "serverrs-qqbot-mysql",
    [switch]$KeepArtifacts
)

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

if ($Container -ne "serverrs-qqbot-mysql") {
    throw "OPS-007 drill only permits the dedicated serverrs-qqbot-mysql container"
}
if (-not (Get-Command docker -ErrorAction SilentlyContinue)) {
    throw "docker command is required"
}

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$databaseRoot = Join-Path $repoRoot "qqbot-server/database"
$baseline = Join-Path $databaseRoot "baseline/20260806_qqbot_schema_v2.sql"
$migrations = Join-Path $databaseRoot "migrations"
$random = [Guid]::NewGuid().ToString("N").Substring(0, 12)
$sourceSchema = "qqbot_accept_ops007_src_$random"
$restoreSchema = "qqbot_accept_ops007_restore_$random"
$remoteBackup = "/tmp/qqbot_ops007_$random.sql"
$tempRoot = Join-Path ([IO.Path]::GetTempPath()) "serverrs-qqbot-ops007-$random"
$backupPath = Join-Path $tempRoot "qqbot-backup.sql"
$exportPath = Join-Path $tempRoot "account-export.jsonl"
$createdSchemas = [Collections.Generic.List[string]]::new()

function Assert-SafeSchema([string]$Schema) {
    if ($Schema -notmatch '^qqbot_accept_ops007_[a-z0-9_]+$' -or $Schema.Length -gt 64) {
        throw "unsafe OPS-007 schema name"
    }
}

function Invoke-Docker([string[]]$Arguments) {
    & docker @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "docker command failed with exit code $LASTEXITCODE"
    }
}

function Invoke-MySql([string]$Sql, [string]$Schema = "") {
    $arguments = @(
        "exec", "-i", $Container, "sh", "-lc",
        'MYSQL_PWD="$MYSQL_ROOT_PASSWORD" exec mysql -uroot --batch --raw --skip-column-names "$@"',
        "ops007-mysql"
    )
    if ($Schema) { $arguments += $Schema }
    $output = $Sql | & docker @arguments
    if ($LASTEXITCODE -ne 0) {
        throw "mysql command failed for isolated OPS-007 schema"
    }
    return @($output)
}

function Apply-SqlFile([string]$Path, [string]$Schema) {
    $sql = [IO.File]::ReadAllText($Path)
    [void](Invoke-MySql -Sql $sql -Schema $Schema)
}

function Scalar([string]$Sql, [string]$Schema) {
    $rows = @(Invoke-MySql -Sql $Sql -Schema $Schema)
    if ($rows.Count -ne 1) {
        throw "expected one scalar row, got $($rows.Count)"
    }
    return [string]$rows[0]
}

Assert-SafeSchema $sourceSchema
Assert-SafeSchema $restoreSchema
$resolvedTempParent = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
$resolvedTempRoot = [IO.Path]::GetFullPath($tempRoot)
if (-not $resolvedTempRoot.StartsWith($resolvedTempParent, [StringComparison]::OrdinalIgnoreCase)) {
    throw "temporary artifact path escaped the system temp directory"
}

try {
    $running = (& docker inspect -f '{{.State.Running}}' $Container 2>$null)
    if ($LASTEXITCODE -ne 0 -or $running -ne "true") {
        throw "serverrs-qqbot-mysql must be running"
    }
    New-Item -ItemType Directory -Path $tempRoot | Out-Null

    foreach ($schema in @($sourceSchema, $restoreSchema)) {
        [void](Invoke-MySql -Sql "CREATE DATABASE ``$schema`` CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci;")
        $createdSchemas.Add($schema)
    }

    Apply-SqlFile -Path $baseline -Schema $sourceSchema
    Get-ChildItem -LiteralPath $migrations -Filter '*.sql' -File |
        Sort-Object Name |
        ForEach-Object { Apply-SqlFile -Path $_.FullName -Schema $sourceSchema }

    $seedSql = @'
INSERT INTO secretary_accounts (source_channel, platform_account_id)
VALUES ('napcat', 'ops007-target'), ('napcat', 'ops007-control');
INSERT INTO secretary_conversations (account_id, conversation_kind, platform_conversation_id)
SELECT id, 'group', CONCAT('ops007-group-', platform_account_id)
FROM secretary_accounts;
INSERT INTO secretary_source_events
  (source_event_id, account_id, conversation_id, source_channel, platform_event_id,
   event_type, actor_platform_id, actor_kind, message_role, occurred_at_unix_secs,
   processing_status, received_at)
SELECT '70000000-0000-0000-0000-000000000001', a.id, c.id, 'napcat',
       'ops007-target-event', 'message', 'ops007-target-actor', 'external',
       'external_observation', 1, 'processed', NOW(6)
FROM secretary_accounts a JOIN secretary_conversations c ON c.account_id = a.id
WHERE a.platform_account_id = 'ops007-target';
INSERT INTO secretary_source_events
  (source_event_id, account_id, conversation_id, source_channel, platform_event_id,
   event_type, actor_platform_id, actor_kind, message_role, occurred_at_unix_secs,
   processing_status, received_at)
SELECT '70000000-0000-0000-0000-000000000002', a.id, c.id, 'napcat',
       'ops007-control-event', 'message', 'ops007-control-actor', 'external',
       'external_observation', 2, 'processed', NOW(6)
FROM secretary_accounts a JOIN secretary_conversations c ON c.account_id = a.id
WHERE a.platform_account_id = 'ops007-control';
INSERT INTO secretary_message_contents
  (source_event_id, normalized_text, segments, mentioned_actor_ids, mention_all, content_mode)
VALUES
  ('70000000-0000-0000-0000-000000000001', 'ops007-target-private-export', JSON_ARRAY(), JSON_ARRAY(), 0, 'normal'),
  ('70000000-0000-0000-0000-000000000002', 'ops007-control-preserved', JSON_ARRAY(), JSON_ARRAY(), 0, 'normal');
'@
    [void](Invoke-MySql -Sql $seedSql -Schema $sourceSchema)

    $canonicalSql = @'
SELECT CONCAT_WS('|', a.source_channel, a.platform_account_id, c.conversation_kind,
  c.platform_conversation_id, e.source_event_id, e.platform_event_id, m.normalized_text)
FROM secretary_accounts a
JOIN secretary_conversations c ON c.account_id = a.id
JOIN secretary_source_events e ON e.account_id = a.id AND e.conversation_id = c.id
JOIN secretary_message_contents m ON m.source_event_id = e.source_event_id
ORDER BY a.platform_account_id, e.source_event_id;
'@
    $sourceCanonical = @(Invoke-MySql -Sql $canonicalSql -Schema $sourceSchema) -join "`n"

    Invoke-Docker @(
        "exec", $Container, "sh", "-lc",
        'MYSQL_PWD="$MYSQL_ROOT_PASSWORD" mysqldump -uroot --single-transaction --hex-blob --skip-comments --set-gtid-purged=OFF "$1" > "$2"',
        "ops007-dump", $sourceSchema, $remoteBackup
    )
    Invoke-Docker @("cp", "${Container}:$remoteBackup", $backupPath)
    if ((Get-Item -LiteralPath $backupPath).Length -le 0) {
        throw "backup artifact is empty"
    }
    $dump = [IO.File]::ReadAllText($backupPath)
    [void](Invoke-MySql -Sql $dump -Schema $restoreSchema)
    $restoredCanonical = @(Invoke-MySql -Sql $canonicalSql -Schema $restoreSchema) -join "`n"
    if ($restoredCanonical -cne $sourceCanonical) {
        throw "restored canonical data differs from source"
    }
    $sourceObjects = Scalar "SELECT COUNT(*) FROM information_schema.TABLES WHERE TABLE_SCHEMA = DATABASE();" $sourceSchema
    $restoredObjects = Scalar "SELECT COUNT(*) FROM information_schema.TABLES WHERE TABLE_SCHEMA = DATABASE();" $restoreSchema
    if ($sourceObjects -ne $restoredObjects) {
        throw "restored schema object count differs from source"
    }

    $exportSql = @'
SELECT JSON_OBJECT(
  'version', 1,
  'source_channel', a.source_channel,
  'account_ref', a.platform_account_id,
  'conversation_kind', c.conversation_kind,
  'conversation_ref', c.platform_conversation_id,
  'source_event_ref', e.source_event_id,
  'platform_event_ref', e.platform_event_id,
  'occurred_at_unix_secs', e.occurred_at_unix_secs,
  'content_mode', m.content_mode,
  'normalized_text', m.normalized_text,
  'segments', m.segments
)
FROM secretary_accounts a
JOIN secretary_conversations c ON c.account_id = a.id
JOIN secretary_source_events e ON e.account_id = a.id AND e.conversation_id = c.id
JOIN secretary_message_contents m ON m.source_event_id = e.source_event_id
WHERE a.source_channel = 'napcat' AND a.platform_account_id = 'ops007-target'
ORDER BY e.occurred_at_unix_secs, e.source_event_id;
'@
    $exportRows = @(Invoke-MySql -Sql $exportSql -Schema $sourceSchema)
    if ($exportRows.Count -ne 1 -or $exportRows[0] -notmatch 'ops007-target-private-export') {
        throw "account export did not contain the expected target record"
    }
    if (($exportRows -join "`n") -match 'ops007-control-preserved') {
        throw "account export crossed the requested account boundary"
    }
    [IO.File]::WriteAllLines($exportPath, $exportRows, [Text.UTF8Encoding]::new($false))

    $targetId = Scalar "SELECT id FROM secretary_accounts WHERE source_channel='napcat' AND platform_account_id='ops007-target';" $sourceSchema
    if ($targetId -notmatch '^[0-9]+$') { throw "invalid target account id" }
    $purgeSql = @"
START TRANSACTION;
DELETE FROM secretary_notification_outbox WHERE command_account_id = $targetId;
DELETE FROM secretary_accounts WHERE id = $targetId;
COMMIT;
"@
    [void](Invoke-MySql -Sql $purgeSql -Schema $sourceSchema)

    $references = @(Invoke-MySql -Sql @'
SELECT TABLE_NAME, COLUMN_NAME
FROM information_schema.COLUMNS
WHERE TABLE_SCHEMA = DATABASE()
  AND COLUMN_NAME IN ('account_id', 'managed_account_id', 'command_account_id')
ORDER BY TABLE_NAME, COLUMN_NAME;
'@ -Schema $sourceSchema)
    foreach ($reference in $references) {
        $parts = $reference -split "`t"
        if ($parts.Count -ne 2 -or $parts[0] -notmatch '^secretary_[a-z0-9_]+$') {
            throw "unexpected account reference metadata"
        }
        $remaining = Scalar "SELECT COUNT(*) FROM ``$($parts[0])`` WHERE ``$($parts[1])`` = $targetId;" $sourceSchema
        if ($remaining -ne "0") {
            throw "account purge left an explicit account reference"
        }
    }
    if ((Scalar "SELECT COUNT(*) FROM secretary_source_events WHERE source_event_id='70000000-0000-0000-0000-000000000001';" $sourceSchema) -ne "0") {
        throw "target source event survived account purge"
    }
    if ((Scalar "SELECT COUNT(*) FROM secretary_message_contents WHERE normalized_text='ops007-target-private-export';" $sourceSchema) -ne "0") {
        throw "target message content survived account purge"
    }
    if ((Scalar "SELECT COUNT(*) FROM secretary_accounts WHERE platform_account_id='ops007-control';" $sourceSchema) -ne "1") {
        throw "control account was modified by target purge"
    }
    if ((Scalar "SELECT COUNT(*) FROM secretary_message_contents WHERE normalized_text='ops007-control-preserved';" $sourceSchema) -ne "1") {
        throw "control account content was modified by target purge"
    }

    Push-Location $repoRoot
    try {
        & cargo test -p qqbot-server `
            'realtime_spool::tests::empty_spool_can_be_rotated_only_after_old_generation_is_removed' `
            -- --exact
        if ($LASTEXITCODE -ne 0) { throw "realtime spool rotation drill failed" }
        & cargo test -p qqbot-server `
            'recall::tests::empty_recall_spool_can_be_rotated_only_after_old_file_is_removed' `
            -- --exact
        if ($LASTEXITCODE -ne 0) { throw "recall spool rotation drill failed" }
    } finally {
        Pop-Location
    }

    Write-Host "OPS-007 PASS: backup/restore, account export/purge, and empty-spool key rotation verified."
    Write-Host "Source objects: $sourceObjects; restored objects: $restoredObjects; exported records: $($exportRows.Count)."
    if ($KeepArtifacts) {
        Write-Host "Synthetic artifacts retained at: $tempRoot"
    }
} finally {
    foreach ($schema in $createdSchemas) {
        Assert-SafeSchema $schema
        try { [void](Invoke-MySql -Sql "DROP DATABASE IF EXISTS ``$schema``;") } catch { Write-Warning $_ }
    }
    try { Invoke-Docker @("exec", $Container, "rm", "-f", $remoteBackup) } catch { Write-Warning $_ }
    if (-not $KeepArtifacts -and (Test-Path -LiteralPath $resolvedTempRoot)) {
        $verified = [IO.Path]::GetFullPath($resolvedTempRoot)
        if (-not $verified.StartsWith($resolvedTempParent, [StringComparison]::OrdinalIgnoreCase) -or
            (Split-Path -Leaf $verified) -notmatch '^serverrs-qqbot-ops007-[a-f0-9]{12}$') {
            throw "refusing to remove an unverified temporary artifact directory"
        }
        Remove-Item -LiteralPath $verified -Recurse -Force
    }
}
