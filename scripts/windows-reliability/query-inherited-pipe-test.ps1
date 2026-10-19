param([string]$Helper,[string]$Node,[switch]$ParentLifetimeControl)
$ErrorActionPreference='Stop'
if ($ParentLifetimeControl -and -not $IsWindows) { throw 'Parent-lifetime control is Windows-specific' }
if ($IsWindows -and -not $ParentLifetimeControl) {
    # Reproduce the old fixture independently before testing the surviving writer.
    & (Join-Path $PSHOME 'pwsh.exe') -NoProfile -File $PSCommandPath -Helper $Helper -Node $Node -ParentLifetimeControl
    if ($LASTEXITCODE -ne 0) { throw 'Windows parent-lifetime control failed' }
}
. $Helper
. (Join-Path $PSScriptRoot query-test-handles.ps1)
$pidFile=[IO.Path]::GetTempFileName(); $releaseFile=$pidFile+'.release'; $readyFile=$pidFile+'.ready'; $leafScript=$pidFile+'.leaf.js'
$script:ownedRoot=$null; $script:ownedLeaf=$null; $script:stdoutReader=$null; $script:stderrReader=$null
$script:rootOwnershipRetained=$false
$script:leafMayHaveStarted=$false; $script:leafOwnershipRetained=$false
$script:leafReadyBeforeRelease=$false; $script:fixtureNode=$null; $script:fixtureUv=$null
$started=[Action[Diagnostics.Process]]{
    param([Diagnostics.Process]$queryProcess)
    $script:ownedRoot=[Diagnostics.Process]::GetProcessById($queryProcess.Id)
    $null=$script:ownedRoot.Handle
    $script:rootOwnershipRetained=$true
    $script:stdoutReader=$queryProcess.StandardOutput; $script:stderrReader=$queryProcess.StandardError
        $script:stdoutHandle=Get-OwnedReaderHandle $script:stdoutReader; $script:stderrHandle=Get-OwnedReaderHandle $script:stderrReader
    $wait=[Diagnostics.Stopwatch]::StartNew()
    while ((Get-Item $pidFile).Length -eq 0) { if ($wait.ElapsedMilliseconds -gt 30000) { throw 'Owned fixture did not publish leaf identity' }; Start-Sleep -Milliseconds 10 }
    $script:ownedLeaf=[Diagnostics.Process]::GetProcessById([int]([IO.File]::ReadAllText($pidFile)))
    $null=$script:ownedLeaf.Handle
    $script:leafOwnershipRetained=$true
    while (-not (Test-Path -LiteralPath $readyFile)) {
        if ($script:ownedLeaf.HasExited) { throw 'Owned leaf exited before readiness' }
        if ($wait.ElapsedMilliseconds -gt 30000) { throw 'Owned leaf did not report readiness' }
        Start-Sleep -Milliseconds 10
    }
    $identity=[IO.File]::ReadAllText($readyFile) | ConvertFrom-Json
    $script:fixtureNode=[string]$identity.node; $script:fixtureUv=[string]$identity.uv
    if ($script:ownedLeaf.HasExited) { throw 'Owned leaf exited before parent release' }
    $script:leafReadyBeforeRelease=$true
    [IO.File]::WriteAllText($releaseFile,'release')
}
$primaryFixtureFailure=$null
try {
    [IO.File]::WriteAllText($leafScript,'const fs=require("node:fs");setInterval(()=>{},1000);setTimeout(()=>process.exit(86),60000);fs.writeFileSync(process.argv[2]+".writing",JSON.stringify({node:process.version,uv:process.versions.uv}));fs.renameSync(process.argv[2]+".writing",process.argv[2]);')
    $code='const fs=require("node:fs"),cp=require("node:child_process");const c=cp.spawn(process.execPath,[process.argv[3],process.argv[4]],{detached:process.argv[5]==="detached",stdio:["ignore","inherit","inherit"]});fs.writeFileSync(process.argv[1]+".writing",String(c.pid));fs.renameSync(process.argv[1]+".writing",process.argv[1]);c.unref();const t=setInterval(()=>{if(fs.existsSync(process.argv[2])){clearInterval(t);process.exit(0)}},10);'
    # libuv's Windows non-detached job kills its children when the parent exits.
    # A detached, explicitly owned writer is required to retain these inherited pipes.
    $mode=if ($ParentLifetimeControl) { 'parent-lifetime' } else { 'detached' }
    # Startup can fail before the callback retains the detached child handle.
    # Its 60-second self-expiry bounds orphan lifetime, never a passing assertion.
    $script:leafMayHaveStarted=$true
    $result=[CovenNativeQuery]::RunForTests($Node,@('-e',$code,$pidFile,$releaseFile,$leafScript,$readyFile,$mode),30000,1024,$null,$started)
    $controlLeafExited=$null
    if ($ParentLifetimeControl -and $null -ne $script:ownedLeaf) {
        # Wait for the control's natural job-driven exit, never kill to satisfy it.
        $controlLeafExited=$script:ownedLeaf.WaitForExit(30000)
    }
    # Preserve bounded native outcomes before assertions so a platform-specific
    # fixture or reader failure can be diagnosed without raw process output.
    @{
        parentLifetimeControl=[bool]$ParentLifetimeControl; controlLeafExited=$controlLeafExited; leafReadyBeforeRelease=$script:leafReadyBeforeRelease
        fixtureNode=$script:fixtureNode; fixtureUv=$script:fixtureUv
        leafExitCode=$(if ($null -ne $script:ownedLeaf -and $script:ownedLeaf.HasExited) { $script:ownedLeaf.ExitCode } else { $null })
        started=$result.Started; rootExited=$result.RootExited; readersSettled=$result.ReadersSettled
        reaped=$result.Reaped; timedOut=$result.TimedOut; outputLimited=$result.OutputLimited
        exitCode=$result.ExitCode; elapsedMs=$result.ElapsedMs
        failure=$result.Failure; cleanupFailure=$result.CleanupFailure; readerFailure=$result.ReaderFailure
        leafHasExited=$(if ($null -ne $script:ownedLeaf) { $script:ownedLeaf.HasExited } else { $null })
        stdoutHandleClosed=$(if ($null -ne $script:stdoutHandle) { $script:stdoutHandle.IsClosed } else { $null })
        stderrHandleClosed=$(if ($null -ne $script:stderrHandle) { $script:stderrHandle.IsClosed } else { $null })
    } | ConvertTo-Json
    if (-not $script:leafReadyBeforeRelease) { throw 'Fixture writer was not ready before parent exit' }
    if ($ParentLifetimeControl) {
        if ($result.Failure -ne '' -or $result.ExitCode -ne 0 -or -not $result.RootExited -or -not $result.ReadersSettled -or -not $controlLeafExited) { throw 'Non-detached control did not reproduce parent-lifetime cleanup' }
        if ($script:ownedLeaf.ExitCode -eq 86) { throw 'Control leaf reached fixture self-expiry instead of parent-lifetime cleanup' }
        if (-not $script:stdoutHandle.IsClosed -or -not $script:stderrHandle.IsClosed) { throw 'Control left reader handles open' }
        return
    }
    if ($result.Failure -ne 'InheritedPipesRemainOpen' -or -not $result.RootExited -or -not $result.ReadersSettled) { throw 'Inherited pipe cleanup did not retain separate outcomes' }
    if ($script:ownedLeaf.HasExited) { throw 'Fixture did not retain its inherited-pipe writer' }
    if (-not $script:stdoutHandle.IsClosed -or -not $script:stderrHandle.IsClosed) { throw 'Inherited pipe left reader handles open' }
    @{ rootExited=$result.RootExited; readersSettled=$result.ReadersSettled; failure=$result.Failure; retainedOwnedHandles=$true; descendantCleanupClaimed=$false } | ConvertTo-Json
} catch {
    $primaryFixtureFailure=$_.Exception.GetType().Name
    throw
} finally {
    $cleanup=Clear-OwnedInheritedFixture -UnownedLeafPossible ($script:leafMayHaveStarted -and -not $script:leafOwnershipRetained) -Readers @($script:stdoutReader,$script:stderrReader) -Processes @($(if ($script:leafOwnershipRetained) { $script:ownedLeaf } else { $null }),$(if ($script:rootOwnershipRetained) { $script:ownedRoot } else { $null })) -Files @($pidFile,$releaseFile,$readyFile,$leafScript,($pidFile+'.writing'),($readyFile+'.writing'))
    if ($cleanup.Failures.Count -ne 0) {
        @{ fixtureCleanupFailed=$true; allOwnedExited=$cleanup.AllExited; failures=$cleanup.Failures; primaryFixtureFailure=$primaryFixtureFailure } | ConvertTo-Json
        if ($null -eq $primaryFixtureFailure) { throw 'Owned fixture cleanup reported failures after attempting every resource' }
    }
}
