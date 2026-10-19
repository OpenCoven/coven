$ErrorActionPreference='Stop'
. "$PSScriptRoot/query-helper.ps1"
. "$PSScriptRoot/query-plan.ps1"
. "$PSScriptRoot/query-summary.ps1"
. "$PSScriptRoot/query-executor.ps1"
. "$PSScriptRoot/provider-schema.ps1"
. "$PSScriptRoot/preflight-core.ps1"
try {
 Invoke-CovenNativeMetadataPreflight -WindowsHost $false -Discover { throw 'DISCOVERY SHOULD NOT RUN' }
 throw 'Unsupported host accepted'
} catch { if ($_.Exception.Message -ne 'Windows host required') { throw } }
$r=Invoke-CovenNativeMetadataPreflight -WindowsHost $true -Discover { @{} } -InvokeQuery { throw 'QUERY SHOULD NOT RUN' }
if ($r.public.queries.Count -ne 13 -or $r.public.captureAttempted -or $r.public.metadataComplete) { throw 'Incorrect preflight scope' }
if ($r.public.formatVersion -ne 3) { throw 'Observation schema version lost' }
if ($r.public.clock.frequency -le 0 -or $r.public.clock.after -lt $r.public.clock.before) { throw 'Clock evidence invalid' }
if (($r.public | ConvertTo-Json -Depth 10) -match 'rawResults|Stdout|Stderr|SHOULD NOT RUN') { throw 'Internal results leaked' }
'preflight orchestration PASS'

$schema='<publisherMetadata name="Microsoft-Windows-Kernel-File" guid="{EDD08927-9CC4-4E65-B970-C2560FB5C289}"><events><event value="21" version="1"><template><data name="Irp" inType="win:Pointer"/></template></event></events></publisherMetadata>'
$withSchema=Invoke-CovenNativeMetadataPreflight -WindowsHost $true -Discover { @{wevtutil=[pscustomobject]@{path='/synthetic/wevtutil.exe';summary=@{present=$true}}} } -InvokeQuery {
 param($path,$arguments,$timeout,$limit)
 [pscustomobject]@{Started=$true;TimedOut=$false;OutputLimited=$false;Reaped=$true;RootExited=$true;ReadersSettled=$true;ExitCode=0;ElapsedMs=1;Stdout=$schema;Stderr='';Failure='';CleanupFailure='';ReaderFailure='';Phases=[CovenQueryPhases]::new()}
}
if ($withSchema.public.providerSchemas.Count -ne 2 -or -not $withSchema.public.providerSchemas[0].schema.providerMatched) { throw 'Provider schema not projected' }
if ($withSchema.public.metadataComplete) { throw 'Partial schemas claimed complete' }
'preflight schema projection PASS'
