param([string]$Helper, [string]$Node)
$ErrorActionPreference = 'Stop'
. $Helper
$normal = [CovenNativeQuery]::Run($Node, @('-e', 'process.stdout.write("ok");process.stderr.write("diagnostic");process.exitCode=7'), 10000, 1024)
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
@{ normalExit=$normal.ExitCode; boundedChars=$large.Stdout.Length; boundedStderrChars=$stderrCap.Stderr.Length; outputLimited=$large.OutputLimited; timedOut=$timeout.TimedOut; stdinClosed=($stdin.Stdout -eq 'eof'); missingToolFailure=$missing.Failure; allOwnedRootsReaped=($normal.Reaped -and $large.Reaped -and $timeout.Reaped -and $stderrCap.Reaped -and $stdin.Reaped) } | ConvertTo-Json
