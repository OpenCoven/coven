param([string]$Helper,[string]$Node)
$ErrorActionPreference='Stop'
. $Helper
. (Join-Path $PSScriptRoot query-test-handles.ps1)
$releaseFile=Join-Path ([IO.Path]::GetTempPath()) ('coven-query-release-'+[Guid]::NewGuid().ToString('N'))
$script:ownedRoot=$null; $script:stdoutReader=$null; $script:stderrReader=$null
$started=[Action[Diagnostics.Process]]{
    param([Diagnostics.Process]$queryProcess)
    $script:ownedRoot=[Diagnostics.Process]::GetProcessById($queryProcess.Id)
    $null=$script:ownedRoot.Handle
    $script:stdoutReader=$queryProcess.StandardOutput
    $script:stderrReader=$queryProcess.StandardError
        $script:stdoutHandle=Get-OwnedReaderHandle $script:stdoutReader; $script:stderrHandle=Get-OwnedReaderHandle $script:stderrReader
    [IO.File]::WriteAllText($releaseFile,'release')
}
try {
    $result=[CovenNativeQuery]::RunForTests($Node,@('-e','const fs=require("node:fs");const t=setInterval(()=>{if(fs.existsSync(process.argv[1])){clearInterval(t);process.stdout.write("ok")}},10);',$releaseFile),10000,1024,$null,$started)
    if ($result.ExitCode -ne 0 -or $result.Stdout -ne 'ok' -or -not $result.RootExited -or -not $result.ReadersSettled) { throw 'Normal root/reader outcome failed' }
    if (-not $script:stdoutHandle.IsClosed -or -not $script:stderrHandle.IsClosed) { throw 'Owned reader handles remained open after query completion' }
    @{ stdoutHandleClosed=$script:stdoutHandle.IsClosed; stderrHandleClosed=$script:stderrHandle.IsClosed; stdoutCanReadAfterDispose=$script:stdoutReader.BaseStream.CanRead; streamType=$script:stdoutReader.BaseStream.GetType().FullName } | ConvertTo-Json
} finally {
    if (Test-Path -LiteralPath $releaseFile) { Remove-Item -LiteralPath $releaseFile }
    if ($null -ne $script:stdoutReader) { $script:stdoutReader.Dispose() }
    if ($null -ne $script:stderrReader) { $script:stderrReader.Dispose() }
    if ($null -ne $script:ownedRoot) { if (-not $script:ownedRoot.HasExited) { $script:ownedRoot.Kill($true); if (-not $script:ownedRoot.WaitForExit(3000)) { throw 'Owned root cleanup failed' } }; $script:ownedRoot.Dispose() }
}
