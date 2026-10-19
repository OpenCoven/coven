$ErrorActionPreference='Stop'
. "$PSScriptRoot/query-helper.ps1"
. "$PSScriptRoot/query-plan.ps1"
. "$PSScriptRoot/query-summary.ps1"
. "$PSScriptRoot/query-executor.ps1"
$missing=Invoke-CovenPreflightQueries -Identities @{} -InvokeQuery { throw 'Missing tool invoked' }
if ($missing.summaries.Count -ne 13 -or @($missing.summaries | Where-Object available).Count -ne 0) { throw 'Missing queries lost' }
$calls=[Collections.Generic.List[object]]::new()
$ids=@{ xperf=[pscustomobject]@{path='/synthetic/xperf.exe'} }
$result=Invoke-CovenPreflightQueries -Identities $ids -InvokeQuery {
 param($path,$arguments,$timeout,$limit)
 $calls.Add(@($path,$arguments,$timeout,$limit))
 [pscustomobject]@{Started=$true;TimedOut=$false;OutputLimited=$false;Reaped=$false;RootExited=$false;ReadersSettled=$true;ExitCode=$null;ElapsedMs=2;Stdout='SECRET';Stderr='PRIVATE';Failure='cleanup';CleanupFailure='PRIVATE';ReaderFailure='';Phases=[CovenQueryPhases]::new()}
}
if ($calls.Count -ne 1 -or -not $result.aborted) { throw 'Unproved cleanup did not stop queries' }
if ($calls[0][2] -ne 10000 -or $calls[0][3] -ne 524288) { throw 'Unbounded query arguments' }
if (($result.summaries | ConvertTo-Json -Depth 8) -match 'SECRET|PRIVATE|/synthetic') { throw 'Raw data published' }
if ($result.summaries.Count -ne 13 -or $result.rawResults.Count -ne 1) { throw 'Aborted query accounting incomplete' }
'query executor PASS'
$thrown=Invoke-CovenPreflightQueries -Identities $ids -InvokeQuery { throw 'SECRET launch boundary exception' }
if (-not $thrown.aborted -or $thrown.summaries.Count -ne 13 -or -not $thrown.summaries[0].invocationThrew) { throw 'Invocation exception lost accounting' }
if (($thrown.summaries | ConvertTo-Json -Depth 8) -match 'SECRET') { throw 'Invocation exception leaked' }
'query executor exception PASS'
