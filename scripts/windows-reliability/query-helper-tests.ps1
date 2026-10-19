param([string]$Helper, [string]$Node, [string]$SummaryOutput)
$ErrorActionPreference = 'Stop'
. $Helper
$normal = [CovenNativeQuery]::Run($Node, @('-e', 'process.stdout.write("ok");process.stderr.write("diagnostic");process.exitCode=7'), 10000, 1024)
if ($null -eq $normal.Phases -or $normal.Phases.Frequency -le 0) { throw 'Query phase clock missing' }
if ($normal.ExitCode -ne 7 -or $normal.Stdout -ne 'ok' -or $normal.Stderr -ne 'diagnostic' -or $normal.TimedOut -or $normal.OutputLimited -or -not $normal.Reaped) { throw 'Normal query result did not preserve exit/output/ownership' }
$large = [CovenNativeQuery]::Run($Node, @('-e', 'process.stdout.write("x".repeat(1000000));setInterval(()=>{},1000)'), 10000, 1024)
if (-not $large.OutputLimited -or $large.Stdout.Length -gt 1024 -or -not $large.Reaped) { throw 'Output cap did not bound and reap owned query' }
$timeout = [CovenNativeQuery]::Run($Node, @('-e', 'setInterval(()=>{},1000)'), 200, 1024)
if (-not $timeout.TimedOut -or -not $timeout.Reaped) { throw 'Query timeout did not reap owned process' }
$stderrCap = [CovenNativeQuery]::Run($Node, @('-e', 'process.stderr.write("x".repeat(1000000));setInterval(()=>{},1000)'), 10000, 1024)
if (-not $stderrCap.OutputLimited -or $stderrCap.Stderr.Length -gt 1024 -or -not $stderrCap.Reaped) { throw 'Stderr cap did not bound and reap owned query' }
$stdin = [CovenNativeQuery]::Run($Node, @('-e', 'process.stdin.on("end",()=>process.stdout.write("eof"));process.stdin.resume()'), 10000, 1024)
if ($stdin.ExitCode -ne 0 -or $stdin.Stdout -ne 'eof' -or -not $stdin.Reaped) { throw 'Query did not close interactive stdin' }
$missingPath = Join-Path ([IO.Path]::GetTempPath()) ('coven-query-missing-' + [Guid]::NewGuid().ToString('N'))
$missing = [CovenNativeQuery]::Run($missingPath, @(), 10000, 1024)
if ($missing.Started -or -not $missing.Reaped -or $missing.Failure -ne 'Win32Exception' -or $missing.Stdout -ne '' -or $missing.Stderr -ne '') { throw 'Missing tool was not a bounded structured startup failure' }
$ordered=@('StartCallEnteredTicks','StartCallReturnedTicks','StdinCloseEnteredTicks','StdinCloseReturnedTicks',
 'ReaderSetupEnteredTicks','ReaderSetupReturnedTicks','MonitorEnteredTicks','TimeoutObservedTicks','CleanupEnteredTicks',
 'KillCallEnteredTicks','KillCallReturnedTicks','RootWaitEnteredTicks','RootWaitReturnedTicks',
 'InitialReaderWaitEnteredTicks','InitialReaderWaitReturnedTicks','FallbackReaderWaitEnteredTicks','FallbackReaderWaitReturnedTicks',
 'ReaderDisposalAttemptsEnteredTicks','ReaderDisposalAttemptsFinishedTicks','CleanupFinishedTicks')
foreach ($result in @($normal,$large,$timeout,$stderrCap,$stdin,$missing)) {
 $previous=0L
 foreach ($name in $ordered) {
  $tick=$result.Phases.$name
  if ($null -ne $tick) {
   if ($tick -isnot [long] -or $tick -lt $previous) { throw 'Query phase ordering or type invalid' }
   $previous=$tick
  }
 }
 if ($null -eq $result.Phases.StartCallEnteredTicks -or $null -eq $result.Phases.CleanupFinishedTicks) { throw 'Returned query lost phase boundaries' }
 if (($null -ne $result.Phases.TimeoutObservedTicks) -ne $result.TimedOut) { throw 'Timeout observation state inconsistent' }
}
if ($null -eq $normal.Phases.StartCallReturnedTicks -or $null -eq $normal.Phases.ReaderSetupReturnedTicks -or $null -eq $normal.Phases.MonitorEnteredTicks -or $null -ne $normal.Phases.KillCallEnteredTicks) { throw 'Successful query phase path invalid' }
if ($null -eq $timeout.Phases.TimeoutObservedTicks -or $null -eq $timeout.Phases.KillCallEnteredTicks -or $null -eq $timeout.Phases.RootWaitReturnedTicks) { throw 'Genuine timeout phase path lost' }
if ($null -ne $missing.Phases.StartCallReturnedTicks -or $null -ne $missing.Phases.StdinCloseEnteredTicks -or $null -ne $missing.Phases.MonitorEnteredTicks -or $null -ne $missing.Phases.RootWaitEnteredTicks) { throw 'Start failure invented post-start calls' }
if ($SummaryOutput) {
 . "$PSScriptRoot/query-summary.ps1"
 $summaries=@($normal,$large,$timeout,$stderrCap,$stdin,$missing | ForEach-Object { ConvertTo-CovenQuerySummary -Id 'xperf-start-help' -Result $_ })
 [IO.File]::WriteAllText($SummaryOutput,($summaries | ConvertTo-Json -Depth 8))
}
@{ normalExit=$normal.ExitCode; boundedChars=$large.Stdout.Length; boundedStderrChars=$stderrCap.Stderr.Length; outputLimited=$large.OutputLimited; timedOut=$timeout.TimedOut; stdinClosed=($stdin.Stdout -eq 'eof'); missingToolFailure=$missing.Failure; allOwnedRootsReaped=($normal.Reaped -and $large.Reaped -and $timeout.Reaped -and $stderrCap.Reaped -and $stdin.Reaped) } | ConvertTo-Json
