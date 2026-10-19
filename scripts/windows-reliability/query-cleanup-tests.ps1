param([string]$Helper,[string]$Node,[string[]]$Faults=@('after-start','reader'))
$ErrorActionPreference='Stop'
. $Helper
. (Join-Path $PSScriptRoot query-test-handles.ps1)
foreach ($fault in $Faults) {
    $script:ownedRoot=$null; $script:stdoutReader=$null; $script:stderrReader=$null
    $started=[Action[Diagnostics.Process]]{
        param([Diagnostics.Process]$queryProcess)
        $script:ownedRoot=[Diagnostics.Process]::GetProcessById($queryProcess.Id)
        $null=$script:ownedRoot.Handle
        $script:stdoutReader=$queryProcess.StandardOutput
        $script:stderrReader=$queryProcess.StandardError
        $script:stdoutHandle=Get-OwnedReaderHandle $script:stdoutReader; $script:stderrHandle=Get-OwnedReaderHandle $script:stderrReader
    }
    try {
        $result=[CovenNativeQuery]::RunForTests($Node,@('-e','setInterval(()=>{},1000)'),1000,1024,$fault,$started)
        if ($result.Failure -eq '' -or -not $result.RootExited -or -not $result.ReadersSettled) { throw 'Fault did not retain structured cleanup evidence' }
        if (-not $script:stdoutHandle.IsClosed -or -not $script:stderrHandle.IsClosed) { throw 'Fault left owned reader handles open' }
    } finally {
        if ($null -ne $script:stdoutReader) { $script:stdoutReader.Dispose() }
        if ($null -ne $script:stderrReader) { $script:stderrReader.Dispose() }
        if ($null -ne $script:ownedRoot) { if (-not $script:ownedRoot.HasExited) { $script:ownedRoot.Kill($true); if (-not $script:ownedRoot.WaitForExit(3000)) { throw 'Test-owned root did not exit' } }; $script:ownedRoot.Dispose() }
    }
}
@{ postStartAndReaderFaultsStructured=$true; ownedHandlesRetained=$true; readersClosed=$true } | ConvertTo-Json

# Fixture teardown must attempt the root even when a reader or leaf fails.
function New-CleanupProcess([string]$Name,[bool]$ExitProved,[bool]$ThrowKill=$false) {
    $process=[pscustomobject]@{ Name=$Name; ExitProved=$ExitProved; ThrowKill=$ThrowKill; HasExited=$false }
    $process | Add-Member ScriptMethod Kill { param($Tree) $script:cleanupEvents.Add($this.Name+'-kill'); if ($this.ThrowKill) { throw [InvalidOperationException]::new('Injected fixture kill failure') } }
    $process | Add-Member ScriptMethod WaitForExit { param($Budget) $script:cleanupEvents.Add($this.Name+'-wait'); return $this.ExitProved }
    $process | Add-Member ScriptMethod Dispose { $script:cleanupEvents.Add($this.Name+'-dispose') }
    return $process
}
$badReader=[pscustomobject]@{}
$badReader | Add-Member ScriptMethod Dispose { throw [InvalidOperationException]::new('Injected fixture disposal failure') }
foreach ($leafExits in @($true,$false)) {
    $script:cleanupEvents=[Collections.Generic.List[string]]::new()
    $fixtureFile=[IO.Path]::GetTempFileName()
    try {
        $leaf=New-CleanupProcess 'leaf' $leafExits (-not $leafExits); $root=New-CleanupProcess 'root' $true
        $cleanup=Clear-OwnedInheritedFixture -Readers @($badReader) -Processes @($leaf,$root) -Files @($fixtureFile)
        if ($cleanup.Failures.Count -eq 0) { throw 'Reader cleanup failure was lost' }
        foreach ($event in @('leaf-kill','leaf-wait','root-kill','root-wait','root-dispose')) {
            if (-not $script:cleanupEvents.Contains($event)) { throw 'Cleanup stopped before attempting every owned process' }
        }
        if ($cleanup.AllExited -ne $leafExits) { throw 'Unproved process exit was misclassified' }
        if ((Test-Path -LiteralPath $fixtureFile) -ne (-not $leafExits)) { throw 'Fixture files were removed without exit proof' }
        if ($script:cleanupEvents.Contains('leaf-dispose') -ne $leafExits) { throw 'Unproved process ownership was disposed' }
    } finally { if (Test-Path -LiteralPath $fixtureFile) { Remove-Item -LiteralPath $fixtureFile } }
}
@{ independentFixtureCleanup=$true; unprovedExitRetainsFilesAndHandle=$true } | ConvertTo-Json

# A root exit cannot prove cleanup of a leaf whose ownership was never retained.
$script:cleanupEvents=[Collections.Generic.List[string]]::new()
$uncertainFile=[IO.Path]::GetTempFileName()
try {
    $root=New-CleanupProcess 'root' $true
    $cleanup=Clear-OwnedInheritedFixture -Readers @() -Processes @($null,$root) -Files @($uncertainFile) -UnownedLeafPossible $true
    if ($cleanup.AllExited -or $cleanup.Failures -notcontains 'LeafOwnershipUnproved') { throw 'Unowned possible leaf was declared reaped' }
    if (-not (Test-Path -LiteralPath $uncertainFile)) { throw 'Unowned possible leaf lost fixture evidence' }
    if (-not $script:cleanupEvents.Contains('root-dispose')) { throw 'Uncertain leaf prevented owned root cleanup' }
} finally { if (Test-Path -LiteralPath $uncertainFile) { Remove-Item -LiteralPath $uncertainFile } }
@{ unownedPossibleLeafRetainsEvidence=$true } | ConvertTo-Json
