param([Parameter(Mandatory)][string]$Directory,[Parameter(Mandatory)][string]$Commit)
$ErrorActionPreference='Stop'
$binding=$null
$payload=$null
$code=2
$failed=$false
$cleanupFailed=$false
try {
    # Execute the same bounded private bytes that the closed inventory later names.
    # A path reread after dot-sourcing would not identify the executed script block.
    $binderPath=Join-Path $PSScriptRoot 'provider-binding.ps1'
    $binderStream=[IO.File]::Open($binderPath,[IO.FileMode]::Open,[IO.FileAccess]::Read,[IO.FileShare]::Read)
    try {
        $binderBuffer=[byte[]]::new(262145)
        $binderUsed=0
        while ($binderUsed -lt $binderBuffer.Length) {
            $count=$binderStream.Read($binderBuffer,$binderUsed,$binderBuffer.Length-$binderUsed)
            if ($count -eq 0) { break }
            $binderUsed+=$count
        }
        if ($binderUsed -eq 0 -or $binderUsed -gt 262144) { throw 'source' }
        $binderBytes=$binderBuffer[0..($binderUsed-1)]
    } finally { $binderStream.Dispose() }
    $binderHash=[Convert]::ToHexString([Security.Cryptography.SHA256]::HashData([byte[]]$binderBytes)).ToLowerInvariant()
    . ([scriptblock]::Create([Text.UTF8Encoding]::new($false,$true).GetString([byte[]]$binderBytes)))
    # Reject unsupported hosts before any provider API or compilation.
    $binding=Open-CovenProviderRuntimeBinding
    $capture=Get-CovenProviderSourceCapture -Directory $Directory -Commit $Commit
    $boundBinder=@($capture.Files | Where-Object { $_.Id -ceq 'binding' })
    if ($boundBinder.Count -ne 1 -or $boundBinder[0].Sha256 -cne $binderHash) { throw 'source' }
    $head=(& git -C $Directory rev-parse HEAD 2>$null)
    if ($LASTEXITCODE -ne 0 -or $head -cne $Commit) { throw 'source' }
    Add-CovenProviderCapturedTypes -Capture $capture
    $collector=[CovenProviderBound.CovenKernelReader]::Collect([CovenProviderBound.CovenWindowsKernelApi]::new(),[CovenProviderBound.CovenKernelBuffers]::new())
    $source=ConvertTo-CovenProviderSourceDto -Capture $capture
    $hostProof=ConvertTo-CovenProviderHostDto -Binding $binding
    $encoded=[CovenProviderBound.CovenKernelPublic]::EncodeProbe($collector,$source,$hostProof,[CovenProviderBound.CovenKernelJsonSerializer]::new())
    if ($encoded.Payload.Length -eq 0 -or -not (Test-CovenProviderSourceCapture -Capture $capture) -or
        -not (Test-CovenProviderRuntimeBinding -Binding $binding)) { throw 'binding' }
    $headAfter=(& git -C $Directory rev-parse HEAD 2>$null)
    if ($LASTEXITCODE -ne 0 -or $headAfter -cne $Commit) { throw 'source' }
    $payload=$encoded.Payload
    $code=$encoded.ExitCode
} catch {
    $failed=$true
    $cleanupFailed=$_.Exception.Data['ProviderReleaseFailed'] -eq $true
} finally {
    if ($null -ne $binding) {
        try { Close-CovenProviderRuntimeBinding -Binding $binding } catch { $failed=$true;$cleanupFailed=$true }
    }
}
if ($failed) {
    # Private binding diagnostics never cross the child transport boundary.
    if ($cleanupFailed) { [Console]::Error.Write("provider-binding-cleanup-failed`n") }
    else { [Console]::Error.Write("provider-binding-failed`n") }
    exit 2
}
# One complete capped byte array; no BOM, newline or serialized private capture.
$stdout=[Console]::OpenStandardOutput()
try { $stdout.Write($payload,0,$payload.Length);$stdout.Flush() } finally { $stdout.Dispose() }
exit $code
