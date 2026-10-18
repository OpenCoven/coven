param([string]$Helper,[string]$Node)
$ErrorActionPreference='Stop'
. $Helper
. (Join-Path $PSScriptRoot query-test-handles.ps1)
$pidFile=[IO.Path]::GetTempFileName(); $releaseFile=$pidFile+'.release'
$script:ownedRoot=$null; $script:ownedLeaf=$null; $script:stdoutReader=$null; $script:stderrReader=$null
$started=[Action[Diagnostics.Process]]{
    param([Diagnostics.Process]$queryProcess)
    $script:ownedRoot=[Diagnostics.Process]::GetProcessById($queryProcess.Id)
    $null=$script:ownedRoot.Handle
    $script:stdoutReader=$queryProcess.StandardOutput; $script:stderrReader=$queryProcess.StandardError
        $script:stdoutHandle=Get-OwnedReaderHandle $script:stdoutReader; $script:stderrHandle=Get-OwnedReaderHandle $script:stderrReader
    $wait=[Diagnostics.Stopwatch]::StartNew()
    while ((Get-Item $pidFile).Length -eq 0) { if ($wait.ElapsedMilliseconds -gt 30000) { throw 'Owned fixture did not publish leaf identity' }; Start-Sleep -Milliseconds 10 }
    $script:ownedLeaf=[Diagnostics.Process]::GetProcessById([int]([IO.File]::ReadAllText($pidFile)))
    $null=$script:ownedLeaf.Handle
    [IO.File]::WriteAllText($releaseFile,'release')
}
try {
    $code='const fs=require("node:fs"),cp=require("node:child_process");const c=cp.spawn(process.execPath,["-e","setInterval(()=>{},1000)"],{stdio:["ignore","inherit","inherit"]});fs.writeFileSync(process.argv[1],String(c.pid));c.unref();const t=setInterval(()=>{if(fs.existsSync(process.argv[2])){clearInterval(t);process.exit(0)}},10);'
    $result=[CovenNativeQuery]::RunForTests($Node,@('-e',$code,$pidFile,$releaseFile),30000,1024,$null,$started)
    # Preserve bounded native outcomes before assertions so a platform-specific
    # fixture or reader failure can be diagnosed without raw process output.
    @{
        started=$result.Started; rootExited=$result.RootExited; readersSettled=$result.ReadersSettled
        reaped=$result.Reaped; timedOut=$result.TimedOut; outputLimited=$result.OutputLimited
        exitCode=$result.ExitCode; elapsedMs=$result.ElapsedMs
        failure=$result.Failure; cleanupFailure=$result.CleanupFailure; readerFailure=$result.ReaderFailure
        leafHasExited=$(if ($null -ne $script:ownedLeaf) { $script:ownedLeaf.HasExited } else { $null })
        stdoutHandleClosed=$(if ($null -ne $script:stdoutHandle) { $script:stdoutHandle.IsClosed } else { $null })
        stderrHandleClosed=$(if ($null -ne $script:stderrHandle) { $script:stderrHandle.IsClosed } else { $null })
    } | ConvertTo-Json
    if ($result.Failure -ne 'InheritedPipesRemainOpen' -or -not $result.RootExited -or -not $result.ReadersSettled) { throw 'Inherited pipe cleanup did not retain separate outcomes' }
    if ($script:ownedLeaf.HasExited) { throw 'Fixture did not retain its inherited-pipe writer' }
    if (-not $script:stdoutHandle.IsClosed -or -not $script:stderrHandle.IsClosed) { throw 'Inherited pipe left reader handles open' }
    @{ rootExited=$result.RootExited; readersSettled=$result.ReadersSettled; failure=$result.Failure; retainedOwnedHandles=$true; descendantCleanupClaimed=$false } | ConvertTo-Json
} finally {
    foreach ($reader in @($script:stdoutReader,$script:stderrReader)) { if ($null -ne $reader) { $reader.Dispose() } }
    foreach ($owned in @($script:ownedLeaf,$script:ownedRoot)) { if ($null -ne $owned) { if (-not $owned.HasExited) { $owned.Kill($true); if (-not $owned.WaitForExit(3000)) { throw 'Owned fixture cleanup failed' } }; $owned.Dispose() } }
    foreach ($file in @($pidFile,$releaseFile)) { if (Test-Path -LiteralPath $file) { Remove-Item -LiteralPath $file } }
}
