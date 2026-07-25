param(
    [string]$ReleaseDirectory = (Join-Path $PSScriptRoot '..\release'),
    [string]$ExpectedSubject = $env:REMOTEDECK_SIGNER_SUBJECT
)

$ErrorActionPreference = 'Stop'
$resolvedRelease = (Resolve-Path -LiteralPath $ReleaseDirectory).Path

$targets = @(
    @{
        Name = 'packaged application'
        Files = @(Join-Path $resolvedRelease 'win-unpacked\RemoteDeck.exe')
    },
    @{
        Name = 'NSIS installer'
        Files = @(Get-ChildItem -LiteralPath $resolvedRelease -Filter 'RemoteDeck-*-win-x64-setup.exe' -File | ForEach-Object FullName)
    },
    @{
        Name = 'portable executable'
        Files = @(Get-ChildItem -LiteralPath $resolvedRelease -Filter 'RemoteDeck-*-win-x64-portable.exe' -File | ForEach-Object FullName)
    }
)

$results = foreach ($target in $targets) {
    if ($target.Files.Count -ne 1) {
        throw "Expected exactly one $($target.Name), found $($target.Files.Count)."
    }

    $path = $target.Files[0]
    $signature = Get-AuthenticodeSignature -LiteralPath $path
    if ($signature.Status -ne [System.Management.Automation.SignatureStatus]::Valid) {
        throw "Invalid Authenticode signature for $path`: $($signature.Status) — $($signature.StatusMessage)"
    }
    if ($null -eq $signature.SignerCertificate) {
        throw "Authenticode signer certificate is missing for $path."
    }
    if ($null -eq $signature.TimeStamperCertificate) {
        throw "RFC 3161 timestamp is missing for $path."
    }
    if (-not [string]::IsNullOrWhiteSpace($ExpectedSubject) -and
        $signature.SignerCertificate.Subject -notlike "*$ExpectedSubject*") {
        throw "Unexpected signer for $path`: $($signature.SignerCertificate.Subject)"
    }

    [pscustomobject]@{
        Artifact = $target.Name
        File = [IO.Path]::GetFileName($path)
        Status = [string]$signature.Status
        Subject = $signature.SignerCertificate.Subject
        Thumbprint = $signature.SignerCertificate.Thumbprint
        TimestampAuthority = $signature.TimeStamperCertificate.Subject
        TimestampNotAfter = $signature.TimeStamperCertificate.NotAfter.ToUniversalTime().ToString('o')
    }
}

$results | Format-Table -AutoSize
Write-Host "PASS: verified $($results.Count) Authenticode signatures and RFC 3161 timestamps."
