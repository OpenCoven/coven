param([Parameter(Mandatory)][string]$Directory,[Parameter(Mandatory)][string]$Commit,
    [Parameter(Mandatory)][string]$ProbeSourceHash)
$ErrorActionPreference='Stop'
$PSNativeCommandUseErrorActionPreference=$false
$binding=$null;$payload=$null;$code=2;$failed=$false;$cleanupFailed=$false
$stage='bootstrap';$failureStage='none'
try {
    # Execute bounded private bytes, then identify those same bytes in capture.
    $binderPath=Join-Path $Directory 'scripts/windows-reliability/token-binding.ps1'
    $stream=[IO.File]::Open($binderPath,[IO.FileMode]::Open,[IO.FileAccess]::Read,[IO.FileShare]::Read)
    try {
        $buffer=[byte[]]::new(262145);$used=0
        while($used -lt $buffer.Length){
            $count=$stream.Read($buffer,$used,$buffer.Length-$used)
            if($count -eq 0){break}
            $used+=$count
        }
        if($used -eq 0 -or $used -gt 262144){throw 'source'}
        $binderBytes=[byte[]]$buffer[0..($used-1)]
    } finally {$stream.Dispose()}
    $binderHash=[Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($binderBytes)).ToLowerInvariant()
    . ([scriptblock]::Create([Text.UTF8Encoding]::new($false,$true).GetString($binderBytes)))
    $stage='runtime-binding'
    $binding=Open-CovenTokenRuntimeBinding
    $stage='source-binding'
    $capture=Get-CovenTokenSourceCapture -Directory $Directory -Commit $Commit
    $boundBinder=@($capture.Files | Where-Object {$_.Id -ceq 'binding'})
    if($boundBinder.Count -ne 1 -or $boundBinder[0].Sha256 -cne $binderHash){throw 'source'}
    $boundProbe=@($capture.Files | Where-Object {$_.Id -ceq 'probe'})
    if($ProbeSourceHash -cnotmatch '^[0-9a-f]{64}$' -or $boundProbe.Count -ne 1 -or $boundProbe[0].Sha256 -cne $ProbeSourceHash){throw 'source'}
    $head=(& git -C $Directory rev-parse HEAD 2>$null)
    if($LASTEXITCODE -ne 0 -or $head -cne $Commit){throw 'source'}
    $dirty=@(& git -C $Directory status --porcelain=v1 --untracked-files=all 2>$null)
    if($LASTEXITCODE -ne 0 -or $dirty.Count -ne 0){throw 'source'}
    $stage='compilation'
    Add-CovenTokenCapturedTypes -Capture $capture
    if(-not (Test-CovenTokenSourceCapture $capture) -or -not (Test-CovenTokenRuntimeBinding $binding)){throw 'binding'}
    $stage='collection'
    $result=[CovenTokenBound.CovenOwnedTokenCollector]::Collect()
    $stage='identity-projection'
    $source=ConvertTo-CovenTokenSourceDto -Capture $capture
    $hostProof=ConvertTo-CovenTokenHostDto -Binding $binding
    $stage='encoding'
    $encoded=[CovenTokenBound.CovenTokenBindingPublic]::EncodeProbe($result,$source,$hostProof)
    if($encoded.Payload.Length -eq 0 -or $encoded.Payload.Length -gt 32768 -or $encoded.ExitCode -notin @(0,1,2)){throw 'encoding'}
    $stage='final-binding'
    if(-not (Test-CovenTokenSourceCapture $capture) -or -not (Test-CovenTokenRuntimeBinding $binding)){throw 'binding'}
    $headAfter=(& git -C $Directory rev-parse HEAD 2>$null)
    if($LASTEXITCODE -ne 0 -or $headAfter -cne $Commit){throw 'source'}
    $dirtyAfter=@(& git -C $Directory status --porcelain=v1 --untracked-files=all 2>$null)
    if($LASTEXITCODE -ne 0 -or $dirtyAfter.Count -ne 0){throw 'source'}
    $payload=$encoded.Payload;$code=$encoded.ExitCode
} catch {
    $failed=$true;$failureStage=$stage
    $cleanupFailed=$_.Exception.Data['TokenReleaseFailed'] -eq $true
} finally {
    if($null -ne $binding){
        try {Close-CovenTokenRuntimeBinding -Binding $binding}
        catch {
            if(-not $failed){$failureStage='reference-release'}
            $failed=$true;$cleanupFailed=$true
        }
    }
}
if($failed){
    # Closed stage labels only. Never serialize private errors, capture or paths.
    $cleanup=if($cleanupFailed){'true'}else{'false'}
    [Console]::Error.Write("token-probe-failed stage=$failureStage cleanupFailed=$cleanup`n")
    exit 2
}
# Only a complete capped payload after all owned reference cleanup has succeeded.
$stdout=[Console]::OpenStandardOutput()
try {$stdout.Write($payload,0,$payload.Length);$stdout.Flush()}
catch {[Console]::Error.Write("token-probe-failed stage=transport cleanupFailed=false`n");exit 2}
finally {$stdout.Dispose()}
exit $code
