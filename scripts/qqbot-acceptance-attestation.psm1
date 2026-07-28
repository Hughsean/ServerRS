Set-StrictMode -Version Latest

function Get-QqbotAcceptanceSignaturePayload {
    param([Parameter(Mandatory = $true)]$AttestationDocument)

    return [ordered]@{
        issuer = [string]$AttestationDocument.issuer
        key_id = [string]$AttestationDocument.key_id
        issued_at = [string]$AttestationDocument.issued_at
        protected_environment = [string]$AttestationDocument.protected_environment
        git_head = [string]$AttestationDocument.git_head
        git_tree = [string]$AttestationDocument.git_tree
        git_dirty = [string]$AttestationDocument.git_dirty
        matrix_sha256 = [string]$AttestationDocument.matrix_sha256
        run_report_sha256 = [string]$AttestationDocument.run_report_sha256
        attestations = @($AttestationDocument.attestations | ForEach-Object { [ordered]@{
            check_id = [string]$_.check_id
            approved_evidence = [string]$_.approved_evidence
            test_file_sha256 = [string]$_.test_file_sha256
            run_report_sha256 = [string]$_.run_report_sha256
        } })
    }
}

function Test-QqbotAcceptanceRsaPssAttestation {
    param(
        [Parameter(Mandatory = $true)]$AttestationDocument,
        [Parameter(Mandatory = $true)][string]$TrustedPublicKeyPath
    )

    $signaturePayload = Get-QqbotAcceptanceSignaturePayload -AttestationDocument $AttestationDocument
    if ([string]::IsNullOrWhiteSpace($signaturePayload.key_id) -or
        [string]::IsNullOrWhiteSpace($signaturePayload.issued_at)) {
        throw "Attestation requires key_id and issued_at"
    }

    $canonicalPayload = $signaturePayload | ConvertTo-Json -Compress -Depth 100
    $computedPayloadHash = [Convert]::ToHexString(
        [Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes($canonicalPayload))
    ).ToLowerInvariant()
    if ($computedPayloadHash -ne [string]$AttestationDocument.signed_payload_sha256) {
        throw "Attestation signed payload hash is not bound to its claims"
    }
    if ([string]::IsNullOrWhiteSpace([string]$AttestationDocument.signature) -or
        [string]$AttestationDocument.signature_algorithm -ne "rsa-pss-sha256" -or
        [string]::IsNullOrWhiteSpace([string]$AttestationDocument.signed_payload_sha256)) {
        throw "Attestation requires an RSA-PSS-SHA256 signature and signed payload hash"
    }

    $rsa = $null
    try {
        $publicKeyPem = Get-Content -LiteralPath $TrustedPublicKeyPath -Raw -Encoding UTF8
        $rsa = [Security.Cryptography.RSA]::Create()
        $rsa.ImportFromPem($publicKeyPem)
        $hashBytes = [Convert]::FromHexString($computedPayloadHash)
        $signatureBytes = [Convert]::FromBase64String([string]$AttestationDocument.signature)
        if (-not $rsa.VerifyHash(
            $hashBytes,
            $signatureBytes,
            [Security.Cryptography.HashAlgorithmName]::SHA256,
            [Security.Cryptography.RSASignaturePadding]::Pss
        )) {
            throw "Attestation signature verification failed"
        }
    }
    finally {
        if ($null -ne $rsa) { $rsa.Dispose() }
    }

    return [pscustomobject]@{
        canonical_payload = $canonicalPayload
        payload_sha256 = $computedPayloadHash
    }
}

Export-ModuleMember -Function Get-QqbotAcceptanceSignaturePayload, Test-QqbotAcceptanceRsaPssAttestation
