using namespace System.Security.Cryptography.X509Certificates
function Test-CovenMicrosoftSigner {
    param($SubjectName)
    try {
        if ($SubjectName -isnot [X500DistinguishedName]) { return $false }
        $organizations=[Collections.Generic.List[string]]::new()
        foreach ($rdn in $SubjectName.EnumerateRelativeDistinguishedNames()) {
            # Ambiguous multi-valued RDNs are not admitted by this bounded policy.
            if ($rdn.HasMultipleElements) { return $false }
            if ($rdn.GetSingleElementType().Value -ceq '2.5.4.10') {
                $organizations.Add($rdn.GetSingleElementValue())
            }
        }
        return ($organizations.Count -eq 1 -and $organizations[0] -ceq 'Microsoft Corporation')
    } catch { return $false }
}

function Test-CovenToolAdmission {
    param([string]$Tool,[string]$SignatureStatus,[bool]$MicrosoftSigner,[string]$OriginalFilename)
    $SignatureStatus -ceq 'Valid' -and $MicrosoftSigner -and $OriginalFilename -ieq "$Tool.exe"
}

function Get-CovenToolIdentity {
    param(
        [Parameter(Mandatory)][ValidateSet('xperf','tracelog','tracerpt','WPAExporter','wevtutil','whoami')][string]$Tool,
        [Parameter(Mandatory)][string]$AbsolutePath,
        [Parameter(Mandatory)][ValidateSet('path','wpt','system32')][string]$LocationKind
    )
    if (-not [IO.Path]::IsPathFullyQualified($AbsolutePath) -or
        [IO.Path]::GetFileName($AbsolutePath) -ine "$Tool.exe") { throw 'Invalid tool path' }
    $summary=[ordered]@{ tool=$Tool; locationKind=$LocationKind; present=$false; identityFailed=$false; sha256=$null; fileVersion=$null; signatureStatus='unavailable'; microsoftSigner=$false; originalFilenameMatched=$false; admitted=$false }
    if (-not (Test-Path -LiteralPath $AbsolutePath -PathType Leaf)) {
        return [pscustomobject]@{ path=$null; summary=[pscustomobject]$summary }
    }
    try {
        $item=Get-Item -LiteralPath $AbsolutePath -ErrorAction Stop
        $summary.sha256=(Get-FileHash -LiteralPath $item.FullName -Algorithm SHA256 -ErrorAction Stop).Hash.ToLowerInvariant()
        $v=$item.VersionInfo
        # Use numeric resource fields; free-form vendor/version strings may contain paths.
        $summary.fileVersion=@([int]$v.FileMajorPart,[int]$v.FileMinorPart,[int]$v.FileBuildPart,[int]$v.FilePrivatePart)
        $summary.present=$true
        if (Get-Command Get-AuthenticodeSignature -ErrorAction SilentlyContinue) {
            try {
                $signature=Get-AuthenticodeSignature -LiteralPath $item.FullName -ErrorAction Stop
                $status=[string]$signature.Status
                $summary.microsoftSigner=($null -ne $signature.SignerCertificate -and (Test-CovenMicrosoftSigner -SubjectName $signature.SignerCertificate.SubjectName))
                if ($status -in @('Valid','UnknownError','NotSigned','HashMismatch','NotTrusted','NotSupportedFileFormat','Incompatible')) { $summary.signatureStatus=$status }
            } catch { $summary.signatureStatus='query-failed' }
        }
        $summary.originalFilenameMatched=($v.OriginalFilename -ieq "$Tool.exe")
        $stableHash=(Get-FileHash -LiteralPath $item.FullName -Algorithm SHA256 -ErrorAction Stop).Hash.ToLowerInvariant()
        $summary.admitted=($stableHash -ceq $summary.sha256 -and (Test-CovenToolAdmission -Tool $Tool -SignatureStatus $summary.signatureStatus -MicrosoftSigner $summary.microsoftSigner -OriginalFilename $v.OriginalFilename))
        return [pscustomobject]@{ path=$(if($summary.admitted){$item.FullName}else{$null}); summary=[pscustomobject]$summary }
    } catch {
        $summary.identityFailed=$true
        return [pscustomobject]@{ path=$null; summary=[pscustomobject]$summary }
    }
}
