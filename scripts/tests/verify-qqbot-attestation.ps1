Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

Import-Module (Join-Path $PSScriptRoot "..\qqbot-acceptance-attestation.psm1") -Force

function Assert-ThrowsMessage {
    param(
        [Parameter(Mandatory = $true)][scriptblock]$Action,
        [Parameter(Mandatory = $true)][string]$ExpectedMessage
    )

    try {
        & $Action
    }
    catch {
        if ($_.Exception.Message -ne $ExpectedMessage) {
            throw "Expected error '$ExpectedMessage', got '$($_.Exception.Message)'"
        }
        return
    }
    throw "Expected error '$ExpectedMessage', but action succeeded"
}

function New-TestAttestation {
    return [pscustomobject]@{
        issuer = "release-hardening-test"
        key_id = "ephemeral-test-key"
        issued_at = "2026-07-27T00:00:00Z"
        protected_environment = "true"
        git_head = "1111111111111111111111111111111111111111"
        git_tree = "2222222222222222222222222222222222222222"
        git_dirty = "false"
        matrix_sha256 = "3333333333333333333333333333333333333333333333333333333333333333"
        run_report_sha256 = "4444444444444444444444444444444444444444444444444444444444444444"
        attestations = @([pscustomobject]@{
            check_id = "B7-HEALTH-001-RUNTIME"
            approved_evidence = "L4"
            test_file_sha256 = "5555555555555555555555555555555555555555555555555555555555555555"
            run_report_sha256 = "4444444444444444444444444444444444444444444444444444444444444444"
        })
        signature_algorithm = "rsa-pss-sha256"
        signed_payload_sha256 = ""
        signature = ""
    }
}

$temp = Join-Path ([System.IO.Path]::GetTempPath()) "qqbot-rsa-pss-$([Guid]::NewGuid().ToString('N'))"
$rsa = $null
try {
    [System.IO.Directory]::CreateDirectory($temp) | Out-Null
    $publicKeyPath = Join-Path $temp "public.pem"
    $rsa = [Security.Cryptography.RSA]::Create(2048)
    [System.IO.File]::WriteAllText($publicKeyPath, $rsa.ExportSubjectPublicKeyInfoPem())

    $document = New-TestAttestation
    $payload = Get-QqbotAcceptanceSignaturePayload -AttestationDocument $document
    $canonical = $payload | ConvertTo-Json -Compress -Depth 100
    $hashBytes = [Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes($canonical))
    $document.signed_payload_sha256 = [Convert]::ToHexString($hashBytes).ToLowerInvariant()
    $document.signature = [Convert]::ToBase64String($rsa.SignHash(
        $hashBytes,
        [Security.Cryptography.HashAlgorithmName]::SHA256,
        [Security.Cryptography.RSASignaturePadding]::Pss
    ))

    $verified = Test-QqbotAcceptanceRsaPssAttestation `
        -AttestationDocument $document `
        -TrustedPublicKeyPath $publicKeyPath
    if ($verified.payload_sha256 -ne $document.signed_payload_sha256) {
        throw "Valid RSA-PSS attestation returned an unexpected payload hash"
    }

    $tamperedSignature = $document.PSObject.Copy()
    $signatureBytes = [Convert]::FromBase64String($tamperedSignature.signature)
    $signatureBytes[0] = $signatureBytes[0] -bxor 1
    $tamperedSignature.signature = [Convert]::ToBase64String($signatureBytes)
    Assert-ThrowsMessage -ExpectedMessage "Attestation signature verification failed" -Action {
        Test-QqbotAcceptanceRsaPssAttestation `
            -AttestationDocument $tamperedSignature `
            -TrustedPublicKeyPath $publicKeyPath
    }

    $tamperedClaim = $document.PSObject.Copy()
    $tamperedClaim.git_tree = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    Assert-ThrowsMessage -ExpectedMessage "Attestation signed payload hash is not bound to its claims" -Action {
        Test-QqbotAcceptanceRsaPssAttestation `
            -AttestationDocument $tamperedClaim `
            -TrustedPublicKeyPath $publicKeyPath
    }

    Write-Host "RSA-PSS attestation tests passed: valid, signature tamper, claim tamper"
}
finally {
    if ($null -ne $rsa) { $rsa.Dispose() }
    if (Test-Path -LiteralPath $temp) {
        Remove-Item -LiteralPath $temp -Recurse -Force
    }
}
