[CmdletBinding()]
param(
    [string]$MatrixPath = "docs/qq-personal-secretary/acceptance/qqbot-acceptance-v1.json",
    [string]$DockerContainer = "serverrs-qqbot-mysql",
    [string]$DatabaseUrl,
    [string]$OutputDirectory = "target/qqbot-acceptance",
    [string]$EvidenceAttestationPath,
    [switch]$SkipBaseline,
    [switch]$ListOnly,
    [switch]$KeepDatabase,
    [switch]$AllowExpectedFailures
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

Import-Module (Join-Path $PSScriptRoot "qqbot-acceptance-attestation.psm1") -Force

$repositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
Set-Location $repositoryRoot

function Resolve-RepositoryPath {
    param([Parameter(Mandatory = $true)][string]$Path)

    if ([System.IO.Path]::IsPathRooted($Path)) {
        return [System.IO.Path]::GetFullPath($Path)
    }
    return [System.IO.Path]::GetFullPath((Join-Path $repositoryRoot $Path))
}

$matrixFullPath = Resolve-RepositoryPath $MatrixPath
if (-not (Test-Path -LiteralPath $matrixFullPath -PathType Leaf)) {
    throw "Acceptance matrix does not exist: $matrixFullPath"
}

$outputFullPath = Resolve-RepositoryPath $OutputDirectory
$logsPath = Join-Path $outputFullPath "logs"
New-Item -ItemType Directory -Force -Path $logsPath | Out-Null
Get-ChildItem -LiteralPath $logsPath -Filter "*.log" -File -ErrorAction SilentlyContinue |
    Remove-Item -Force

$matrix = Get-Content -LiteralPath $matrixFullPath -Raw -Encoding UTF8 | ConvertFrom-Json -Depth 100
$matrixSha256 = (Get-FileHash -LiteralPath $matrixFullPath -Algorithm SHA256).Hash.ToLowerInvariant()
$attestations = @{}
$attestationFullPath = $null
if ($EvidenceAttestationPath) {
    if (-not [System.IO.Path]::IsPathRooted($EvidenceAttestationPath)) {
        throw "EvidenceAttestationPath must be an absolute path supplied by protected CI"
    }
    $attestationFullPath = [System.IO.Path]::GetFullPath($EvidenceAttestationPath)
    $repositoryPrefix = $repositoryRoot.TrimEnd('\', '/') + [System.IO.Path]::DirectorySeparatorChar
    if ($attestationFullPath.StartsWith($repositoryPrefix, [StringComparison]::OrdinalIgnoreCase)) {
        throw "Evidence attestation must be outside the repository working tree"
    }
    if (-not (Test-Path -LiteralPath $attestationFullPath -PathType Leaf)) {
        throw "Evidence attestation does not exist: $attestationFullPath"
    }
    $expectedProtectedIssuer = $env:QQBOT_ACCEPTANCE_TRUSTED_ISSUER
    $trustedAttestationPublicKeyPath = $env:QQBOT_ACCEPTANCE_TRUSTED_PUBLIC_KEY_PATH
    if (-not $expectedProtectedIssuer -or -not $trustedAttestationPublicKeyPath) {
        throw "Protected runner trust root is unavailable; L4/L5 attestation cannot be accepted locally"
    }
    if (-not [System.IO.Path]::IsPathRooted($trustedAttestationPublicKeyPath)) {
        throw "Protected runner public-key path must be absolute"
    }
    $trustedKeyPath = [System.IO.Path]::GetFullPath($trustedAttestationPublicKeyPath)
    if ($trustedKeyPath.StartsWith($repositoryPrefix, [StringComparison]::OrdinalIgnoreCase) -or
        -not (Test-Path -LiteralPath $trustedKeyPath -PathType Leaf)) {
        throw "Trusted attestation public key must exist outside the repository working tree"
    }
    $attestationDocument = Get-Content -LiteralPath $attestationFullPath -Raw -Encoding UTF8 |
        ConvertFrom-Json -Depth 100
    $currentHead = (& git rev-parse HEAD).Trim()
    $currentTree = (& git rev-parse 'HEAD^{tree}').Trim()
    $dirty = -not [string]::IsNullOrWhiteSpace(((& git status --porcelain=v1) -join "`n"))
    if ($dirty) { throw "Attested evidence requires a clean working tree" }
    if ([string]$attestationDocument.issuer -ne $expectedProtectedIssuer) {
        throw "Attestation issuer does not match the protected issuer"
    }
    if ([string]$attestationDocument.protected_environment -ne "true") {
        throw "Attestation was not issued by a protected environment"
    }
    if ([string]$attestationDocument.git_head -ne $currentHead -or
        [string]$attestationDocument.git_tree -ne $currentTree -or
        [string]$attestationDocument.matrix_sha256 -ne $matrixSha256) {
        throw "Attestation is not bound to this candidate commit/tree/matrix"
    }
    $signaturePayload = Get-QqbotAcceptanceSignaturePayload -AttestationDocument $attestationDocument
    $canonicalPayload = $signaturePayload | ConvertTo-Json -Compress -Depth 100
    $computedPayloadHash = [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes($canonicalPayload))).ToLowerInvariant()
    $verification = Test-QqbotAcceptanceRsaPssAttestation `
        -AttestationDocument $attestationDocument `
        -TrustedPublicKeyPath $trustedKeyPath
    if ([string]$attestationDocument.git_dirty -ne "false") {
        throw "Attestation must assert git_dirty=false"
    }
    if ([string]::IsNullOrWhiteSpace([string]$attestationDocument.run_report_path) -or
        [string]::IsNullOrWhiteSpace([string]$attestationDocument.run_report_sha256)) {
        throw "Attestation must bind an external acceptance run report and SHA-256"
    }
    if (-not [System.IO.Path]::IsPathRooted([string]$attestationDocument.run_report_path)) {
        throw "Attested run_report_path must be absolute and outside the repository"
    }
    $runReportPath = [System.IO.Path]::GetFullPath([string]$attestationDocument.run_report_path)
    if ($runReportPath.StartsWith($repositoryPrefix, [StringComparison]::OrdinalIgnoreCase) -or
        -not (Test-Path -LiteralPath $runReportPath -PathType Leaf)) {
        throw "Attested run report must exist outside the repository working tree"
    }
    $runReportHash = (Get-FileHash -LiteralPath $runReportPath -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($runReportHash -ne [string]$attestationDocument.run_report_sha256) {
        throw "Attested run report SHA-256 does not match"
    }
    foreach ($attestation in @($attestationDocument.attestations)) {
        if ([string]::IsNullOrWhiteSpace([string]$attestation.check_id) -or
            [string]::IsNullOrWhiteSpace([string]$attestation.test_file_sha256) -or
            [string]::IsNullOrWhiteSpace([string]$attestation.run_report_sha256)) {
            throw "Each attested check requires check_id, test_file_sha256, and run_report_sha256"
        }
        if ([string]$attestation.run_report_sha256 -ne $runReportHash) {
            throw "Attested check $($attestation.check_id) is not bound to the verified run report"
        }
        $attestations[[string]$attestation.check_id] = $attestation
    }
}
if ($matrix.schema_version -ne 1) {
    throw "Unsupported acceptance matrix schema_version: $($matrix.schema_version)"
}

if (@($matrix.gate.accepted_statuses).Count -ne 1 -or [string]$matrix.gate.accepted_statuses[0] -ne "PASS") {
    throw "gate.accepted_statuses must be exactly ['PASS']"
}
if (-not [bool]$matrix.gate.missing_test_is_failure -or -not [bool]$matrix.gate.blocked_test_is_failure) {
    throw "missing and blocked acceptance checks must remain failures"
}

$validSeverities = @("P0", "P1", "P2", "P3")
$evidenceRanks = @{ L1 = 1; L2 = 2; L3 = 3; L4 = 4; L5 = 5; L6 = 6 }
$seenRequirementIds = [System.Collections.Generic.HashSet[string]]::new()
$seenCheckIds = [System.Collections.Generic.HashSet[string]]::new()

foreach ($requirement in $matrix.requirements) {
    if (-not $seenRequirementIds.Add([string]$requirement.id)) {
        throw "Duplicate requirement id: $($requirement.id)"
    }
    if ($validSeverities -notcontains [string]$requirement.severity) {
        throw "Invalid severity for $($requirement.id): $($requirement.severity)"
    }
    if (-not $evidenceRanks.ContainsKey([string]$requirement.minimum_evidence)) {
        throw "Invalid minimum evidence for $($requirement.id): $($requirement.minimum_evidence)"
    }
    if (@($requirement.checks).Count -eq 0) {
        throw "Requirement has no checks: $($requirement.id)"
    }
    foreach ($check in $requirement.checks) {
        if (-not $seenCheckIds.Add([string]$check.id)) {
            throw "Duplicate check id: $($check.id)"
        }
        if ([string]$check.kind -ne "cargo_test") {
            throw "Unsupported check kind for $($check.id): $($check.kind)"
        }
        if (-not $evidenceRanks.ContainsKey([string]$check.evidence)) {
            throw "Invalid evidence for $($check.id): $($check.evidence)"
        }
    }
}

function Get-EffectiveEvidence {
    param(
        [Parameter(Mandatory = $true)]$Check,
        [Parameter(Mandatory = $true)][string]$TestFileHash
    )

    $automatic = if ([bool]$Check.requires_mysql) { "L3" } else { "L1" }
    $requested = [string]$Check.evidence
    if ($evidenceRanks[$requested] -le $evidenceRanks[$automatic]) {
        return $requested
    }
    if (-not $attestations.ContainsKey([string]$Check.id)) {
        return $automatic
    }
    $attestation = $attestations[[string]$Check.id]
    $approved = [string]$attestation.approved_evidence
    if (-not $evidenceRanks.ContainsKey($approved)) {
        throw "Invalid approved_evidence in attestation for $($Check.id): $approved"
    }
    if ([string]$attestation.test_file_sha256 -ne $TestFileHash) {
        return $automatic
    }
    if ($evidenceRanks[$approved] -gt $evidenceRanks[$requested]) {
        return $requested
    }
    return $approved
}

function Get-SafeLogName {
    param([Parameter(Mandatory = $true)][string]$Value)
    return ($Value -replace '[^A-Za-z0-9_.-]', '_')
}

function Invoke-LoggedCommand {
    param(
        [Parameter(Mandatory = $true)][string]$Id,
        [Parameter(Mandatory = $true)][string]$Program,
        [Parameter(Mandatory = $true)][object[]]$Arguments
    )

    $logFile = Join-Path $logsPath "$(Get-SafeLogName $Id).log"
    [System.IO.File]::WriteAllText($logFile, "")
    $startedAt = [DateTimeOffset]::Now
    Write-Host "[$Id] $Program $($Arguments -join ' ')"
    & $Program @Arguments 2>&1 | Tee-Object -FilePath $logFile | Out-Host
    $exitCode = $LASTEXITCODE
    $finishedAt = [DateTimeOffset]::Now
    return [pscustomobject]@{
        id = $Id
        exit_code = $exitCode
        started_at = $startedAt.ToString("o")
        finished_at = $finishedAt.ToString("o")
        duration_ms = [int64]($finishedAt - $startedAt).TotalMilliseconds
        log = [System.IO.Path]::GetRelativePath($repositoryRoot, $logFile).Replace('\', '/')
    }
}

function Get-DatabaseNameFromUrl {
    param([Parameter(Mandatory = $true)][string]$Url)

    $withoutQuery = $Url.Split('?', 2)[0].TrimEnd('/')
    return $withoutQuery.Substring($withoutQuery.LastIndexOf('/') + 1)
}

function New-DockerAcceptanceDatabase {
    param([Parameter(Mandatory = $true)][string]$Container)

    $containerStatus = & docker inspect $Container --format '{{.State.Status}}' 2>$null
    if ($LASTEXITCODE -ne 0 -or $containerStatus.Trim() -ne "running") {
        throw "Docker MySQL container is not running: $Container"
    }

    $environment = & docker inspect $Container --format '{{range .Config.Env}}{{println .}}{{end}}'
    if ($LASTEXITCODE -ne 0) {
        throw "Unable to inspect Docker container: $Container"
    }
    $passwordLine = $environment | Where-Object { $_ -like 'MYSQL_ROOT_PASSWORD=*' } | Select-Object -First 1
    if (-not $passwordLine) {
        throw "MYSQL_ROOT_PASSWORD is not available inside Docker container: $Container"
    }
    $rootPassword = $passwordLine.Substring('MYSQL_ROOT_PASSWORD='.Length)

    $portOutput = & docker port $Container '3306/tcp' | Select-Object -First 1
    if ($LASTEXITCODE -ne 0 -or -not $portOutput) {
        throw "Unable to resolve mapped MySQL port for $Container"
    }
    $hostPort = ([string]$portOutput).Trim().Split(':')[-1]
    if ($hostPort -notmatch '^\d+$') {
        throw "Unexpected Docker port mapping: $portOutput"
    }

    $schemaName = "qqbot_accept_$([DateTimeOffset]::Now.ToString('yyyyMMddHHmmss'))_$([Guid]::NewGuid().ToString('N').Substring(0, 8))"
    if ($schemaName -notmatch '^qqbot_accept_[A-Za-z0-9_]+$') {
        throw "Unsafe generated schema name: $schemaName"
    }

    # schemaName 已由上面的严格正则限制为安全标识符，避免把反引号传给 sh 后触发命令替换。
    $createSql = "CREATE DATABASE $schemaName CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci"
    $mysqlCommand = "mysql -uroot -p`"`$MYSQL_ROOT_PASSWORD`" -e `"$createSql`""
    & docker exec $Container sh -lc $mysqlCommand | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw "Failed to create isolated acceptance schema: $schemaName"
    }

    $escapedPassword = [Uri]::EscapeDataString($rootPassword)
    return [pscustomobject]@{
        url = "mysql://root:$escapedPassword@127.0.0.1:$hostPort/$schemaName"
        schema = $schemaName
        container = $Container
        owned = $true
    }
}

function Remove-DockerAcceptanceDatabase {
    param([Parameter(Mandatory = $true)]$Database)

    if (-not $Database.owned) {
        return
    }
    $schemaName = [string]$Database.schema
    if ($schemaName -notmatch '^qqbot_accept_[A-Za-z0-9_]+$') {
        throw "Refusing to drop unsafe schema name: $schemaName"
    }
    $dropSql = "DROP DATABASE IF EXISTS $schemaName"
    $mysqlCommand = "mysql -uroot -p`"`$MYSQL_ROOT_PASSWORD`" -e `"$dropSql`""
    & docker exec ([string]$Database.container) sh -lc $mysqlCommand | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw "Failed to remove isolated acceptance schema: $schemaName"
    }
}

$startedAt = [DateTimeOffset]::Now
if ($SkipBaseline -and $EvidenceAttestationPath) {
    throw "SkipBaseline cannot be combined with an attestation or an approval-eligible report"
}
$baselineExecuted = -not $SkipBaseline -and -not $ListOnly
$approvalEligible = $baselineExecuted -and -not $ListOnly
$approvalIneligibilityReason = if (-not $baselineExecuted) {
    "baseline was skipped or not executed"
} elseif ($ListOnly) {
    "ListOnly inventory is not an approval run"
} else {
    $null
}
$baselineResults = [System.Collections.Generic.List[object]]::new()
$checkResults = [System.Collections.Generic.List[object]]::new()
$database = $null
$previousDatabaseUrl = [Environment]::GetEnvironmentVariable("QQBOT_TEST_DATABASE_URL", "Process")

try {
    if (-not $SkipBaseline -and -not $ListOnly) {
        foreach ($baseline in $matrix.baseline_checks) {
            $execution = Invoke-LoggedCommand `
                -Id ([string]$baseline.id) `
                -Program ([string]$baseline.program) `
                -Arguments @($baseline.arguments)
            $baselineResults.Add([pscustomobject]@{
                id = [string]$baseline.id
                status = if ($execution.exit_code -eq 0) { "PASS" } else { "FAIL" }
                exit_code = $execution.exit_code
                duration_ms = $execution.duration_ms
                log = $execution.log
            })
        }
    }

    $needsMysql = @($matrix.requirements.checks | Where-Object { $_.requires_mysql }).Count -gt 0
    if ($needsMysql -and -not $ListOnly) {
        if ($DatabaseUrl) {
            $providedSchema = Get-DatabaseNameFromUrl $DatabaseUrl
            if ($providedSchema -notmatch '^qqbot_accept_[A-Za-z0-9_]+$') {
                throw "Refusing non-isolated DatabaseUrl schema '$providedSchema'; expected qqbot_accept_*"
            }
            $database = [pscustomobject]@{
                url = $DatabaseUrl
                schema = $providedSchema
                container = $null
                owned = $false
            }
        } else {
            $database = New-DockerAcceptanceDatabase -Container $DockerContainer
        }
        [Environment]::SetEnvironmentVariable("QQBOT_TEST_DATABASE_URL", [string]$database.url, "Process")
        Write-Host "Using isolated MySQL schema: $($database.schema)"
    }

    $inventoryCache = @{}
    foreach ($requirement in $matrix.requirements) {
        foreach ($check in $requirement.checks) {
            $testFile = Resolve-RepositoryPath ([string]$check.test_file)
            if (-not (Test-Path -LiteralPath $testFile -PathType Leaf)) {
                $checkResults.Add([pscustomobject]@{
                    requirement_id = [string]$requirement.id
                    check_id = [string]$check.id
                    test_name = [string]$check.test_name
                    requested_evidence = [string]$check.evidence
                    evidence = "L0"
                    test_file_sha256 = $null
                    status = "MISSING"
                    reason = "test file does not exist: $($check.test_file)"
                    duration_ms = 0
                    log = $null
                })
                continue
            }

            $testFileHash = (Get-FileHash -LiteralPath $testFile -Algorithm SHA256).Hash.ToLowerInvariant()
            $effectiveEvidence = Get-EffectiveEvidence -Check $check -TestFileHash $testFileHash

            $inventoryKey = "$($check.package)|$($check.test_target)"
            if (-not $inventoryCache.ContainsKey($inventoryKey)) {
                $inventoryId = "INVENTORY-$($check.package)-$($check.test_target)"
                $inventory = Invoke-LoggedCommand `
                    -Id $inventoryId `
                    -Program "cargo" `
                    -Arguments @("test", "-p", [string]$check.package, "--test", [string]$check.test_target, "--", "--list")
                $names = [System.Collections.Generic.HashSet[string]]::new()
                if ($inventory.exit_code -eq 0) {
                    foreach ($line in Get-Content -LiteralPath (Resolve-RepositoryPath $inventory.log)) {
                        if ($line -match '^(.*): test$') {
                            [void]$names.Add($Matches[1])
                        }
                    }
                }
                $inventoryCache[$inventoryKey] = [pscustomobject]@{
                    exit_code = $inventory.exit_code
                    names = $names
                    log = $inventory.log
                }
            }

            $knownTests = $inventoryCache[$inventoryKey]
            if ($knownTests.exit_code -ne 0 -or -not $knownTests.names.Contains([string]$check.test_name)) {
                $checkResults.Add([pscustomobject]@{
                    requirement_id = [string]$requirement.id
                    check_id = [string]$check.id
                    test_name = [string]$check.test_name
                    requested_evidence = [string]$check.evidence
                    evidence = $effectiveEvidence
                    test_file_sha256 = $testFileHash
                    status = "MISSING"
                    reason = "exact test name was not discovered"
                    duration_ms = 0
                    log = $knownTests.log
                })
                continue
            }

            if ($ListOnly) {
                $checkResults.Add([pscustomobject]@{
                    requirement_id = [string]$requirement.id
                    check_id = [string]$check.id
                    test_name = [string]$check.test_name
                    requested_evidence = [string]$check.evidence
                    evidence = $effectiveEvidence
                    test_file_sha256 = $testFileHash
                    status = "NOT_RUN"
                    reason = "ListOnly mode"
                    duration_ms = 0
                    log = $knownTests.log
                })
                continue
            }
            if ($check.requires_mysql -and -not $database) {
                $checkResults.Add([pscustomobject]@{
                    requirement_id = [string]$requirement.id
                    check_id = [string]$check.id
                    test_name = [string]$check.test_name
                    requested_evidence = [string]$check.evidence
                    evidence = $effectiveEvidence
                    test_file_sha256 = $testFileHash
                    status = "BLOCKED"
                    reason = "isolated MySQL is unavailable"
                    duration_ms = 0
                    log = $null
                })
                continue
            }

            $arguments = [System.Collections.Generic.List[object]]::new()
            foreach ($argument in @(
                "test", "-p", [string]$check.package, "--test", [string]$check.test_target,
                [string]$check.test_name, "--", "--exact", "--test-threads=1"
            )) {
                $arguments.Add($argument)
            }
            if ($check.ignored) {
                $arguments.Add("--ignored")
            }
            $execution = Invoke-LoggedCommand `
                -Id ([string]$check.id) `
                -Program "cargo" `
                -Arguments @($arguments)
            $checkResults.Add([pscustomobject]@{
                requirement_id = [string]$requirement.id
                check_id = [string]$check.id
                test_name = [string]$check.test_name
                requested_evidence = [string]$check.evidence
                evidence = $effectiveEvidence
                test_file_sha256 = $testFileHash
                status = if ($execution.exit_code -eq 0) { "PASS" } else { "FAIL" }
                reason = if ($execution.exit_code -eq 0) { $null } else { "test exited with code $($execution.exit_code)" }
                duration_ms = $execution.duration_ms
                log = $execution.log
            })
        }
    }
}
finally {
    [Environment]::SetEnvironmentVariable("QQBOT_TEST_DATABASE_URL", $previousDatabaseUrl, "Process")
    if ($database -and $database.owned -and -not $KeepDatabase) {
        Remove-DockerAcceptanceDatabase -Database $database
        Write-Host "Removed isolated MySQL schema: $($database.schema)"
    }
}

$requirementResults = [System.Collections.Generic.List[object]]::new()
foreach ($requirement in $matrix.requirements) {
    $checks = @($checkResults | Where-Object { $_.requirement_id -eq [string]$requirement.id })
    $status = "PASS"
    foreach ($candidate in @("FAIL", "MISSING", "BLOCKED", "NOT_RUN")) {
        if ($checks.status -contains $candidate) {
            $status = $candidate
            break
        }
    }
    $passedEvidence = @(
        $checks |
            Where-Object { $_.status -eq "PASS" } |
            ForEach-Object { $evidenceRanks[[string]$_.evidence] }
    )
    $maxEvidenceRank = if ($passedEvidence.Count -gt 0) {
        ($passedEvidence | Measure-Object -Maximum).Maximum
    } else {
        0
    }
    $evidenceReason = $null
    if ($status -eq "PASS" -and $maxEvidenceRank -lt $evidenceRanks[[string]$requirement.minimum_evidence]) {
        $status = "FAIL"
        $evidenceReason = "effective evidence is below minimum; L4/L5 requires an independent hash-bound attestation"
    }
    $requirementResults.Add([pscustomobject]@{
        id = [string]$requirement.id
        title = [string]$requirement.title
        severity = [string]$requirement.severity
        required_for_merge = [bool]$requirement.required_for_merge
        minimum_evidence = [string]$requirement.minimum_evidence
        effective_evidence = if ($maxEvidenceRank -gt 0) { "L$maxEvidenceRank" } else { "L0" }
        status = $status
        reason = $evidenceReason
        checks = $checks
    })
}

$blockingSeverities = @($matrix.gate.blocking_severities)
$blockingRequirements = @(
    $requirementResults |
        Where-Object { $_.required_for_merge -and $blockingSeverities -contains $_.severity }
)
$baselineFailed = -not $baselineExecuted -or @($baselineResults | Where-Object { $_.status -ne "PASS" }).Count -gt 0
$requirementsFailed = @($blockingRequirements | Where-Object { $_.status -ne "PASS" }).Count -gt 0
$mergeGate = if ($approvalEligible -and -not $baselineFailed -and -not $requirementsFailed) { "APPROVED" } else { "REJECTED" }
$finishedAt = [DateTimeOffset]::Now

$gitStatus = (& git status --porcelain=v1) -join "`n"
$report = [ordered]@{
    schema_version = 1
    suite_id = [string]$matrix.suite_id
    generated_at = $finishedAt.ToString("o")
    repository_root = $repositoryRoot.Replace('\', '/')
    git_branch = (& git branch --show-current).Trim()
    git_head = (& git rev-parse HEAD).Trim()
    git_dirty = -not [string]::IsNullOrWhiteSpace($gitStatus)
    git_status_porcelain = $gitStatus
    matrix_sha256 = $matrixSha256
    baseline_executed = $baselineExecuted
    approval_eligible = $approvalEligible
    approval_ineligibility_reason = $approvalIneligibilityReason
    evidence_attestation = if ($attestationFullPath) { $attestationFullPath.Replace('\', '/') } else { $null }
    duration_ms = [int64]($finishedAt - $startedAt).TotalMilliseconds
    database = if ($database) { [ordered]@{ schema = $database.schema; isolated = $true } } else { $null }
    baseline = @($baselineResults)
    requirements = @($requirementResults)
    merge_gate = $mergeGate
}

$jsonPath = Join-Path $outputFullPath "latest.json"
$markdownPath = Join-Path $outputFullPath "latest.md"
$report | ConvertTo-Json -Depth 100 | Set-Content -LiteralPath $jsonPath -Encoding UTF8

$markdown = [System.Collections.Generic.List[string]]::new()
$markdown.Add("# QQBot acceptance report")
$markdown.Add("")
$markdown.Add("- Generated: $($report.generated_at)")
$markdown.Add("- Branch: ``$($report.git_branch)``")
$markdown.Add("- HEAD: ``$($report.git_head)``")
$markdown.Add("- Dirty worktree: ``$($report.git_dirty)``")
$markdown.Add("- Matrix SHA-256: ``$($report.matrix_sha256)``")
$markdown.Add("- Baseline executed: ``$($report.baseline_executed)``")
$markdown.Add("- Approval eligible: ``$($report.approval_eligible)``")
if ($report.approval_ineligibility_reason) { $markdown.Add("- Approval ineligibility: $($report.approval_ineligibility_reason)") }
$markdown.Add("- Merge gate: **$mergeGate**")
$markdown.Add("")
$markdown.Add("| Requirement | Severity | Evidence | Status |")
$markdown.Add("|---|---:|---:|---|")
foreach ($item in $requirementResults) {
    $markdown.Add("| $($item.id) $($item.title) | $($item.severity) | $($item.minimum_evidence) | $($item.status) |")
}
$markdown.Add("")
$markdown.Add("## Checks")
$markdown.Add("")
$markdown.Add("| Check | Test | Requested | Effective | Status | Log |")
$markdown.Add("|---|---|---:|---:|---|---|")
foreach ($item in $checkResults) {
    $log = if ($item.log) { "``$($item.log)``" } else { "-" }
    $markdown.Add("| $($item.check_id) | ``$($item.test_name)`` | $($item.requested_evidence) | $($item.evidence) | $($item.status) | $log |")
}
$markdown | Set-Content -LiteralPath $markdownPath -Encoding UTF8

Write-Host "Acceptance JSON: $jsonPath"
Write-Host "Acceptance report: $markdownPath"
Write-Host "Merge gate: $mergeGate"

if ($mergeGate -ne "APPROVED" -and -not $AllowExpectedFailures) {
    exit 1
}
