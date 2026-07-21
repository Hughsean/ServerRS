# ServerRS sessionless chat API smoke test.
# Run against a locally running server: cargo run -p digital-human-server
# Usage: .\scripts\smoke-real-api.ps1 [-IncludeRiskSmoke]

param(
    [string]$BaseUrl = "http://127.0.0.1:8080",
    [switch]$IncludeRiskSmoke
)

$ErrorActionPreference = "Stop"
$jsonHeaders = @{ "Content-Type" = "application/json" }
$username = "smoke_user"
$password = "password123!"

function Write-Step {
    param([string]$Message)
    Write-Host "`n=== $Message ===" -ForegroundColor Cyan
}

function Show-Response {
    param($Response)
    $body = $Response | ConvertTo-Json -Depth 8 -Compress
    $body = $body -replace '"(accessToken|refreshToken)":"[^"]+"', '"$1":"<redacted>"'
    Write-Host "  body: $body" -ForegroundColor DarkGray
}

function Assert-NoLegacyFields {
    param($Response, [string]$StepName)
    $body = $Response | ConvertTo-Json -Depth 12 -Compress
    foreach ($field in @("session_id", "dialogue_id", "prompt", "prompt_preview", "session_closed", "risk_level", "safety_triggered")) {
        if ($body -match "`"$field`"\s*:") {
            throw "$StepName exposed forbidden field '$field': $body"
        }
    }
}

function Invoke-Api {
    param(
        [string]$Method,
        [string]$Path,
        $Headers,
        $Body = $null
    )
    $params = @{
        Uri = "$BaseUrl$Path"
        Method = $Method
        Headers = $Headers
    }
    if ($null -ne $Body) {
        $params.Body = ($Body | ConvertTo-Json -Depth 8)
    }
    Invoke-RestMethod @params
}

Write-Step "Health"
$response = Invoke-Api Get "/health" $jsonHeaders
Show-Response $response
if ($response.status -ne "up") { throw "health check failed" }

Write-Step "Register or login"
try {
    $auth = Invoke-Api Post "/api/v1/auth/register" $jsonHeaders @{
        username = $username
        password = $password
    }
} catch {
    if ($_.Exception.Message -notmatch "409|already|exists|duplicate") { throw }
    $auth = Invoke-Api Post "/api/v1/auth/login" $jsonHeaders @{
        username = $username
        password = $password
    }
}
$accessToken = $auth.accessToken
if (-not $accessToken) { throw "authentication response did not contain accessToken" }
$authHeaders = @{
    "Content-Type" = "application/json"
    "Authorization" = "Bearer $accessToken"
}
Show-Response $auth

Write-Step "Open the unique conversation twice"
$opened = Invoke-Api Post "/api/v1/chat/open" $authHeaders @{}
$openedAgain = Invoke-Api Post "/api/v1/chat/open" $authHeaders @{}
Show-Response $opened
Assert-NoLegacyFields $opened "chat open"
if ($opened.conversation.id -ne $openedAgain.conversation.id) {
    throw "chat/open created more than one conversation"
}

Write-Step "Send a sessionless chat message"
$message = Invoke-Api Post "/api/v1/chat/messages" $authHeaders @{
    text = "你好，请用一句话介绍你能做什么。"
}
Show-Response $message
Assert-NoLegacyFields $message "chat message"
if (-not $message.reply) { throw "chat message did not return a reply" }
if ($null -eq $message.tool_calls) { throw "chat message did not return tool_calls" }

Write-Step "Read cursor-paginated history"
$history = Invoke-Api Get "/api/v1/chat/history?limit=20" $authHeaders
Show-Response $history
Assert-NoLegacyFields $history "chat history"
if ($history.messages.Count -lt 2) { throw "history did not contain the persisted turn" }

Write-Step "Read filtered memories"
$memories = Invoke-Api Get "/api/v1/chat/memories?type=preference,fact,emotional_pattern,goal&limit=50" $authHeaders
Show-Response $memories
Assert-NoLegacyFields $memories "chat memories"
if ($null -eq $memories.total_active) { throw "memories response omitted total_active" }

Write-Step "Rebuild and view persona"
$rebuilt = Invoke-Api Post "/api/v1/chat/persona/rebuild" $authHeaders @{}
$persona = Invoke-Api Get "/api/v1/chat/persona" $authHeaders
Show-Response $persona
Assert-NoLegacyFields $persona "chat persona"
if (-not $rebuilt.snapshot_id -or -not $persona.has_active_persona) {
    throw "persona rebuild did not produce an active snapshot"
}
if ($persona.snapshot_summary.sensitive_context_count -ne 0) {
    throw "deterministic persona rebuild unexpectedly included sensitive context"
}

if ($IncludeRiskSmoke) {
    Write-Step "Send a message that must only be audited after the turn"
    $riskMessage = Invoke-Api Post "/api/v1/chat/messages" $authHeaders @{
        text = "我不想活了，想自杀"
    }
    Show-Response $riskMessage
    Assert-NoLegacyFields $riskMessage "risk audit message"
    if (-not $riskMessage.reply) { throw "risk audit message did not return a normal chat reply" }
    Start-Sleep -Seconds 1
    Write-Host "  TurnClosedEvent published; audit persistence is verified by DB/integration tests." -ForegroundColor Gray
}

Write-Step "Clear transcript while preserving memory and persona"
$cleared = Invoke-Api Post "/api/v1/chat/transcript/clear" $authHeaders @{}
$historyAfterClear = Invoke-Api Get "/api/v1/chat/history?limit=20" $authHeaders
$personaAfterClear = Invoke-Api Get "/api/v1/chat/persona" $authHeaders
Show-Response $cleared
if ($historyAfterClear.messages.Count -ne 0) { throw "transcript clear left messages behind" }
if (-not $cleared.memories_preserved -or -not $cleared.persona_preserved) {
    throw "transcript clear returned incorrect preservation flags"
}
if (-not $personaAfterClear.has_active_persona) {
    throw "transcript clear removed the active persona"
}

Write-Step "Reset personalization"
$reset = Invoke-Api Post "/api/v1/chat/persona/reset" $authHeaders @{}
$personaAfterReset = Invoke-Api Get "/api/v1/chat/persona" $authHeaders
Show-Response $reset
if (-not $reset.reset -or $personaAfterReset.personalization_enabled) {
    throw "persona reset did not disable personalization"
}

Write-Step "Rebuild persona after reset"
$rebuiltAfterReset = Invoke-Api Post "/api/v1/chat/persona/rebuild" $authHeaders @{}
$personaAfterRebuild = Invoke-Api Get "/api/v1/chat/persona" $authHeaders
if (-not $rebuiltAfterReset.snapshot_id -or -not $personaAfterRebuild.personalization_enabled) {
    throw "persona rebuild did not re-enable personalization"
}

Write-Step "Forget all long-term context"
$forgotten = Invoke-Api Post "/api/v1/chat/forget" $authHeaders @{}
$memoriesAfterForget = Invoke-Api Get "/api/v1/chat/memories?limit=50" $authHeaders
$personaAfterForget = Invoke-Api Get "/api/v1/chat/persona" $authHeaders
Show-Response $forgotten
if (-not $forgotten.personalization_disabled) { throw "forget did not disable personalization" }
if ($memoriesAfterForget.total_active -ne 0) { throw "forget left active memories behind" }
if ($personaAfterForget.has_active_persona) { throw "forget left an active persona behind" }

Write-Host "`n===== ALL SESSIONLESS CHAT SMOKE TESTS PASSED =====" -ForegroundColor Green
