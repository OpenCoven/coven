param([Parameter(Mandatory)][string]$Directory,[Parameter(Mandatory)][string]$Commit,
    [Parameter(Mandatory)][string]$OutputDirectory,[Parameter(Mandatory)][string]$TemporaryRoot,
    [Parameter(Mandatory)][string]$RunnerSourceHash)
$ErrorActionPreference='Stop'
$PSNativeCommandUseErrorActionPreference=$false
$binding=$null;$capture=$null;$query=$null;$summary=$null;$publicationQuery=$null;$publicationSummary=$null
$packetBytes=$null;$bundle=$null;$publication=$null;$code=1
$sourceBound=$false;$hostBound=$false;$released=$false;$published=$false;$failed=$false;$cleanupFailed=$false
$stage='bootstrap';$failureStage='none'
function Assert-TokenHead {
    $head=(& git -C $Directory rev-parse HEAD 2>$null)
    if($LASTEXITCODE -ne 0 -or $head -cne $Commit){throw 'source'}
    $dirty=@(& git -C $Directory status --porcelain=v1 --untracked-files=all 2>$null)
    if($LASTEXITCODE -ne 0 -or $dirty.Count -ne 0){throw 'source'}
}
function Read-TokenPublication {
    param($Result)
    if($Result.GetType() -ne [CovenQueryResult] -or -not $Result.Started -or -not $Result.RootExited -or
        -not $Result.ReadersSettled -or -not $Result.Reaped -or $Result.TimedOut -or $Result.OutputLimited -or
        -not [string]::IsNullOrEmpty($Result.Failure) -or -not [string]::IsNullOrEmpty($Result.CleanupFailure) -or
        -not [string]::IsNullOrEmpty($Result.ReaderFailure) -or -not [string]::IsNullOrEmpty($Result.Stderr) -or
        $Result.ExitCode -notin @(0,1,2) -or $Result.Stdout.Length -gt 129){throw 'publication'}
    $document=[Text.Json.JsonDocument]::Parse([string]$Result.Stdout)
    try {
        if($document.RootElement.ValueKind -ne [Text.Json.JsonValueKind]::Object){throw 'publication'}
        $seen=[Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
        $hash=$null;$size=0
        foreach($property in $document.RootElement.EnumerateObject()){
            if(-not $seen.Add($property.Name)){throw 'publication'}
            if($property.Name -ceq 'sha256'){
                if($property.Value.ValueKind -ne [Text.Json.JsonValueKind]::String){throw 'publication'}
                $hash=$property.Value.GetString()
            } elseif($property.Name -ceq 'bytes'){
                if($property.Value.ValueKind -ne [Text.Json.JsonValueKind]::Number -or -not $property.Value.TryGetInt32([ref]$size)){throw 'publication'}
            } else {throw 'publication'}
        }
        if($seen.Count -ne 2 -or $hash -cnotmatch '^[0-9a-f]{64}$' -or $size -lt 1 -or $size -gt 65536){throw 'publication'}
        [pscustomobject]@{sha256=$hash;bytes=$size}
    } finally {$document.Dispose()}
}
try {
    # This runner itself is invoked from captured bytes by the manual workflow.
    $binderPath=Join-Path $Directory 'scripts/windows-reliability/token-binding.ps1'
    $stream=[IO.File]::Open($binderPath,[IO.FileMode]::Open,[IO.FileAccess]::Read,[IO.FileShare]::Read)
    try {
        $buffer=[byte[]]::new(262145);$used=0
        while($used -lt $buffer.Length){$count=$stream.Read($buffer,$used,$buffer.Length-$used);if($count -eq 0){break};$used+=$count}
        if($used -eq 0 -or $used -gt 262144){throw 'source'}
        $binderBytes=[byte[]]$buffer[0..($used-1)]
    } finally {$stream.Dispose()}
    $binderHash=[Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($binderBytes)).ToLowerInvariant()
    $utf8=[Text.UTF8Encoding]::new($false,$true)
    . ([scriptblock]::Create($utf8.GetString($binderBytes)))
    $stage='runtime-binding';$binding=Open-CovenTokenRuntimeBinding
    $nativeTemp=[IO.Path]::TrimEndingDirectorySeparator([IO.Path]::GetFullPath([IO.Path]::GetTempPath()))
    if(-not [string]::Equals($nativeTemp,[IO.Path]::TrimEndingDirectorySeparator([IO.Path]::GetFullPath($TemporaryRoot)),[StringComparison]::OrdinalIgnoreCase)){throw 'temp'}
    $stage='source-binding';$capture=Get-CovenTokenSourceCapture $Directory $Commit
    foreach($pair in @(@('binding',$binderHash),@('runner',$RunnerSourceHash))){
        $entry=@($capture.Files|Where-Object {$_.Id -ceq $pair[0]})
        if($pair[1] -cnotmatch '^[0-9a-f]{64}$' -or $entry.Count -ne 1 -or $entry[0].Sha256 -cne $pair[1]){throw 'source'}
    }
    Assert-TokenHead
    $stage='output-binding'
    $output=[IO.Path]::GetFullPath($OutputDirectory)
    $relative=[IO.Path]::GetRelativePath($nativeTemp,$output)
    if($relative -cnotmatch '^windows-reliability-token-[A-Za-z0-9-]{1,96}[\/]public$'){throw 'output'}
    $job=[IO.Path]::GetDirectoryName($output)
    $null=Assert-CovenTokenSourceRoot $job;$null=Assert-CovenTokenSourceRoot $output
    $stage='compilation';Add-CovenTokenCapturedTypes $capture
    if($null -ne ('CovenNativeQuery' -as [type]) -or $null -ne ('CovenQueryResult' -as [type])){throw 'preloaded query'}
    foreach($id in @('query-helper','query-summary')){
        $file=@($capture.Files|Where-Object {$_.Id -ceq $id})[0]
        . ([scriptblock]::Create($utf8.GetString($file.Bytes)))
    }
    if(-not (Test-CovenTokenSourceCapture $capture) -or -not (Test-CovenTokenRuntimeBinding $binding)){throw 'binding'}
    $stage='identity-projection';$source=ConvertTo-CovenTokenSourceDto $capture;$hostProof=ConvertTo-CovenTokenHostDto $binding
    $stage='probe-invocation';$invocation=New-CovenTokenProbeInvocation $capture
    $executable=$binding.Managed.Files[0].Path
    if(-not [IO.Path]::IsPathFullyQualified($executable) -or $executable.Length -gt 2048){throw 'executable'}
    $stage='query';$query=[CovenNativeQuery]::Run($executable,$invocation.Arguments,10000,32768)
    $stage='query-projection'
    if($query.GetType() -ne [CovenQueryResult]){throw 'query type'}
    if(-not [string]::IsNullOrEmpty($query.CleanupFailure)){$cleanupFailed=$true}
    $summary=ConvertTo-CovenTokenQuerySummary $query
    if($summary.hasCleanupFailure){$cleanupFailed=$true;throw 'query cleanup'}
    $stage='post-query-binding'
    $sourceBound=Test-CovenTokenSourceCapture $capture;$hostBound=Test-CovenTokenRuntimeBinding $binding
    Assert-TokenHead
    if(-not $sourceBound -or -not $hostBound){throw 'binding'}
    $stage='packet';$packetBytes=New-CovenTokenPrivatePacket $source $hostProof $query
} catch {
    $failed=$true;$failureStage=$stage
    $cleanupFailed=$cleanupFailed -or $_.Exception.Data['TokenReleaseFailed'] -eq $true
} finally {
    if($null -ne $binding){
        try {Close-CovenTokenRuntimeBinding $binding;$released=$true}
        catch {if(-not $failed){$failureStage='reference-release'};$failed=$true;$cleanupFailed=$true}
    }
}
# All selected native references have been released before any publication.
if(-not $failed -and -not $cleanupFailed -and $released){
    try {
        $stage='private-bundle'
        $private=Join-Path $job 'private'
        if(Test-Path -LiteralPath $private){throw 'private'}
        $null=[IO.Directory]::CreateDirectory($private)
        $null=Assert-CovenTokenSourceRoot $private
        $bundle=Join-Path $private 'bundle.json'
        $writer=[IO.File]::Open($bundle,[IO.FileMode]::CreateNew,[IO.FileAccess]::Write,[IO.FileShare]::None)
        try {$writer.Write($packetBytes,0,$packetBytes.Length);$writer.Flush($true)} finally {$writer.Dispose()}
        $stage='publisher-invocation'
        $pythonCommands=@(Get-Command python -CommandType Application -ErrorAction Stop);$python=$pythonCommands[0].Source
        $publisher=New-CovenTokenPublisherInvocation -Capture $capture -Executable $python -BundlePath $bundle -TemporaryRoot $TemporaryRoot -OutputDirectory $OutputDirectory
        $stage='publication';$publicationQuery=[CovenNativeQuery]::Run($python,$publisher.Arguments,10000,1024)
        if($publicationQuery.GetType() -ne [CovenQueryResult]){throw 'query type'}
        if(-not [string]::IsNullOrEmpty($publicationQuery.CleanupFailure)){$cleanupFailed=$true}
        if($publicationQuery.Stderr -cmatch '^token-publication-failed cleanupFailed=true\r?\n$'){$cleanupFailed=$true}
        $publicationSummary=ConvertTo-CovenQueryObservation $publicationQuery
        if($publicationSummary.hasCleanupFailure){$cleanupFailed=$true}
        $publication=Read-TokenPublication $publicationQuery
        if($publicationQuery.ExitCode -ne $query.ExitCode){throw 'exit'}
        $stage='private-cleanup'
        if(Test-Path -LiteralPath $bundle){$cleanupFailed=$true;throw 'private cleanup'}
        $stage='final-source';$sourceBound=Test-CovenTokenSourceCapture $capture;Assert-TokenHead
        if(-not $sourceBound -or $cleanupFailed){throw 'binding'}
        $stage='public-receipt'
        $null=Assert-CovenTokenSourceRoot $job;$null=Assert-CovenTokenSourceRoot $output
        $receipt=Join-Path $output 'receipt.json'
        $attributes=[IO.File]::GetAttributes($receipt)
        $item=Get-Item -LiteralPath $receipt -Force -ErrorAction Stop
        if(($attributes -band ([IO.FileAttributes]::Directory -bor [IO.FileAttributes]::ReparsePoint)) -ne 0 -or -not [string]::IsNullOrEmpty($item.LinkType) -or $item.Length -ne $publication.bytes -or (Get-FileHash -LiteralPath $receipt -Algorithm SHA256).Hash.ToLowerInvariant() -cne $publication.sha256){throw 'receipt'}
        $code=[int]$publicationQuery.ExitCode;$published=$true
    } catch {$failed=$true;$failureStage=$stage}
}
if($null -ne $bundle){
    try {if(Test-Path -LiteralPath $bundle){if(-not $failed){$failureStage='private-cleanup'};$cleanupFailed=$true;$published=$false;$failed=$true}}
    catch {if(-not $failed){$failureStage='private-cleanup'};$cleanupFailed=$true;$published=$false;$failed=$true}
}
$effectiveCode=if($failed -or -not $published){1}else{$code}
$execution=[ordered]@{kind='coven.effective-token-execution.v1';captureAttempted=$false;metadataComplete=$false;
    sourceBound=$sourceBound;hostBound=$hostBound;referencesReleased=$released;cleanupFailed=$cleanupFailed;failureStage=$failureStage;
    query=$summary;publisherQuery=$publicationSummary;publication=$publication;publicationSucceeded=$published;exitCode=$effectiveCode}
try {
    if($env:GITHUB_OUTPUT){
        $executionText=$execution|ConvertTo-Json -Depth 8 -Compress
        [IO.File]::AppendAllText($env:GITHUB_OUTPUT,"execution=$executionText`n",[Text.UTF8Encoding]::new($false))
        if($published -and -not $failed){[IO.File]::AppendAllText($env:GITHUB_OUTPUT,"public-receipt=$([IO.Path]::Combine($OutputDirectory,'receipt.json'))`n",[Text.UTF8Encoding]::new($false))}
    }
    if($published -and -not $failed){Write-Output ('token-publication '+($publication|ConvertTo-Json -Compress));exit $code}
} catch {
    $reportedStage=if($failed){$failureStage}else{'outcome'}
    $cleanup=if($cleanupFailed){'true'}else{'false'}
    [Console]::Error.Write("token-execution-failed stage=$reportedStage cleanupFailed=$cleanup outcomeFailed=true`n");exit 1
}
$cleanup=if($cleanupFailed){'true'}else{'false'}
[Console]::Error.Write("token-execution-failed stage=$failureStage cleanupFailed=$cleanup`n")
exit 1
