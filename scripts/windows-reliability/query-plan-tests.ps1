$ErrorActionPreference='Stop'
. "$PSScriptRoot/query-plan.ps1"
$p=@(Get-CovenReadOnlyQueryPlan)
if ($p.Count -ne 13) { throw 'Unexpected query count' }
if (@($p.id | Sort-Object -Unique).Count -ne 13) { throw 'Duplicate query id' }
foreach ($q in $p) {
 if ($q.tool -notin @('xperf','wevtutil','whoami')) { throw 'Unexpected executable' }
 if ($q.arguments -match '^-start$|^-stop$|^-on$|^-off$|^-setprofint$|^im$|^um$|^sl$|^cl$') { throw 'Mutation in plan' }
}
$schema=@($p | Where-Object id -eq 'kernel-file-schema')[0]
if (($schema.arguments -join ' ') -ne 'gp Microsoft-Windows-Kernel-File /ge:true /gm:true /f:xml') { throw 'Wrong schema query' }
$x=@($p | Where-Object id -eq 'xperf-start-help')[0]
if (($x.arguments -join ' ') -ne '-help start') { throw 'Help became activation' }
'fixed read-only query plan PASS'
