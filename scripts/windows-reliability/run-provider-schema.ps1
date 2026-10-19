param([Parameter(Mandatory)][string]$Directory,[Parameter(Mandatory)][string]$Commit,
    [Parameter(Mandatory)][string]$OutputDirectory,[Parameter(Mandatory)][string]$TemporaryRoot)
$ErrorActionPreference='Stop'
$PSNativeCommandUseErrorActionPreference=$false
$binding=$null;$query=$null;$summary=$null;$capture=$null;$code=1
$sourceBound=$false;$hostBound=$false;$published=$false;$failed=$false;$cleanupFailed=$false
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
    $binding=Open-CovenProviderRuntimeBinding
    $nativeTemp=[IO.Path]::TrimEndingDirectorySeparator([IO.Path]::GetFullPath([IO.Path]::GetTempPath()))
    if (-not [string]::Equals($nativeTemp,[IO.Path]::TrimEndingDirectorySeparator([IO.Path]::GetFullPath($TemporaryRoot)),[StringComparison]::OrdinalIgnoreCase)) { throw 'temp' }
    $capture=Get-CovenProviderSourceCapture -Directory $Directory -Commit $Commit
    $boundBinder=@($capture.Files | Where-Object { $_.Id -ceq 'binding' })
    if ($boundBinder.Count -ne 1 -or $boundBinder[0].Sha256 -cne $binderHash) { throw 'source' }
    $head=(& git -C $Directory rev-parse HEAD 2>$null)
    if ($LASTEXITCODE -ne 0 -or $head -cne $Commit) { throw 'source' }
    $dirty=@(& git -C $Directory status --porcelain=v1 --untracked-files=all 2>$null)
    if ($LASTEXITCODE -ne 0 -or $dirty.Count -ne 0) { throw 'source' }
    Add-CovenProviderCapturedTypes -Capture $capture
    $utf8=[Text.UTF8Encoding]::new($false,$true)
    foreach ($id in @('query-helper','query-summary')) {
        $file=@($capture.Files | Where-Object { $_.Id -ceq $id })[0]
        . ([scriptblock]::Create($utf8.GetString($file.Bytes)))
    }
    if (-not (Test-CovenProviderSourceCapture -Capture $capture)) { throw 'source' }
    $source=ConvertTo-CovenProviderSourceDto -Capture $capture
    $hostProof=ConvertTo-CovenProviderHostDto -Binding $binding
    $probePath=@($capture.Files | Where-Object { $_.Id -ceq 'probe' })[0].Path
    $executable=$binding.Managed.Files[0].Path
    $query=[CovenNativeQuery]::Run($executable,@('-NoLogo','-NoProfile','-NonInteractive','-File',$probePath,'-Directory',$Directory,'-Commit',$Commit),10000,524288)
    $summary=ConvertTo-CovenProviderQuerySummary -Result $query
    # No child output is logged. The independent Python decoder admits or rejects it.
    if (-not (Test-CovenProviderSourceCapture -Capture $capture) -or -not (Test-CovenProviderRuntimeBinding -Binding $binding)) { throw 'binding' }
    $privateRoot=Join-Path $nativeTemp ('coven-provider-probe-'+[Guid]::NewGuid().ToString('N'))
    if (Test-Path -LiteralPath $privateRoot) { throw 'private' }
    $private=Join-Path $privateRoot 'private'
    [IO.Directory]::CreateDirectory($private) | Out-Null
    $packet=[ordered]@{source=$source;host=$hostProof;query=$summary;probe=[string]$query.Stdout}
    $packetBytes=$utf8.GetBytes(($packet | ConvertTo-Json -Depth 16 -Compress))
    if ($packetBytes.Length -gt 1048576) { throw 'private-limit' }
    $bundle=Join-Path $private 'bundle.json'
    $stream=[IO.File]::Open($bundle,[IO.FileMode]::CreateNew,[IO.FileAccess]::Write,[IO.FileShare]::None)
    try { $stream.Write($packetBytes,0,$packetBytes.Length);$stream.Flush($true) } finally { $stream.Dispose() }
    $pythonCommands=@(Get-Command python -CommandType Application -ErrorAction Stop)
    $python=$pythonCommands[0].Source
    $validator=@($capture.Files | Where-Object { $_.Id -ceq 'validator' })[0].Path
    # This script is executed directly as .py, never imported through ambient .pyc.
    $publicationText=& $python -B $validator $bundle $TemporaryRoot $OutputDirectory $Directory 2>$null
    $publicationExit=$LASTEXITCODE
    if ($publicationExit -notin @(0,1,2) -or [string]::IsNullOrEmpty($publicationText)) { throw 'publication' }
    $publication=$publicationText | ConvertFrom-Json -ErrorAction Stop
    if ($publication.sha256 -cnotmatch '^[0-9a-f]{64}$' -or $publication.bytes -le 0 -or $publication.bytes -gt 524288 -or $publication.PSObject.Properties.Name.Count -ne 2) { throw 'publication' }
    if ($publicationExit -ne $query.ExitCode) { throw 'exit' }
    $sourceBound=Test-CovenProviderSourceCapture -Capture $capture
    $hostBound=Test-CovenProviderRuntimeBinding -Binding $binding
    $headAfter=(& git -C $Directory rev-parse HEAD 2>$null)
    if ($LASTEXITCODE -ne 0 -or $headAfter -cne $Commit) { $sourceBound=$false }
    $dirtyAfter=@(& git -C $Directory status --porcelain=v1 --untracked-files=all 2>$null)
    if ($LASTEXITCODE -ne 0 -or $dirtyAfter.Count -ne 0) { $sourceBound=$false }
    if (-not $sourceBound -or -not $hostBound) { throw 'binding' }
    $code=$publicationExit
    $published=$true
} catch {
    $failed=$true;$published=$false
    $cleanupFailed=$_.Exception.Data['ProviderReleaseFailed'] -eq $true
} finally {
    if ($null -ne $binding) {
        try { Close-CovenProviderRuntimeBinding -Binding $binding }
        catch { $failed=$true;$published=$false;$hostBound=$false;$cleanupFailed=$true }
    }
}
$effectiveCode=if ($failed -or -not $published) { 1 } else { $code }
$execution=[ordered]@{kind='coven.kernel-file-execution.v1';captureAttempted=$false;metadataComplete=$false;sourceBound=$sourceBound;hostBound=$hostBound;cleanupFailed=$cleanupFailed;query=$summary;publicationSucceeded=$published;exitCode=$effectiveCode}
if ($env:GITHUB_OUTPUT) {
    $executionText=$execution | ConvertTo-Json -Depth 8 -Compress
    [IO.File]::AppendAllText($env:GITHUB_OUTPUT,"execution=$executionText`n",[Text.UTF8Encoding]::new($false))
}
if ($published -and -not $failed) {
    # Set the artifact output only after verified source/host/query/publication and release.
    if ($env:GITHUB_OUTPUT) {
        [IO.File]::AppendAllText($env:GITHUB_OUTPUT,"public-receipt=$([IO.Path]::Combine($OutputDirectory,'receipt.json'))`n",[Text.UTF8Encoding]::new($false))
    }
    Write-Output ('provider-publication '+($publication | ConvertTo-Json -Compress))
    exit $code
}
if ($cleanupFailed) { [Console]::Error.Write("provider-execution-cleanup-failed`n") }
else { [Console]::Error.Write("provider-execution-failed`n") }
exit 1
