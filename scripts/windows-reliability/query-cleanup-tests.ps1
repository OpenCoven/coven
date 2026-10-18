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
