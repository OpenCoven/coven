$ErrorActionPreference = 'Stop'
. "$PSScriptRoot/query-helper.ps1"
. "$PSScriptRoot/query-summary.ps1"
$raw = [pscustomobject]@{ Started=$true; TimedOut=$false; OutputLimited=$false; Reaped=$true; RootExited=$true; ReadersSettled=$true; ExitCode=0; ElapsedMs=12; Stdout='SECRET /Users/private foreign-session S-1-5-21-999 Flush OperationEnd'; Stderr='SECRET'; Failure='SECRET'; CleanupFailure='SECRET'; ReaderFailure='SECRET' }
$raw | Add-Member NoteProperty Phases ([CovenQueryPhases]::new())
$raw.Phases.StartCallEnteredTicks=0L
$raw.Phases.StartCallReturnedTicks=2L
$raw.Phases | Add-Member NoteProperty Private 'SECRET private path'
$s = ConvertTo-CovenQuerySummary -Id 'kernel-file-schema' -Result $raw
if ($null -eq $s.phases -or $s.phases.unit -cne 'stopwatch-elapsed-ticks' -or $s.phases.frequency -ne [Diagnostics.Stopwatch]::Frequency) { throw 'Phase clock projection missing' }
if ($s.phases.startCallEnteredTicks -cne 0L -or $null -ne $s.phases.fallbackReaderWaitReturnedTicks -or $s.phases.PSObject.Properties.Name.Count -ne 22) { throw 'Phase zero/null or fixed shape lost' }
$j = $s | ConvertTo-Json -Depth 5 -Compress
if ($j -match 'SECRET|/Users|foreign-session|S-1-5-21|OperationEnd') { throw 'Raw query data leaked' }
if ($s.stdoutChars -ne $raw.Stdout.Length -or $s.stderrChars -ne 6) { throw 'Missing output accounting' }
if ($s.stdoutSha256 -notmatch '^[0-9a-f]{64}$') { throw 'Missing output digest' }
if (-not $s.hasFailure -or -not $s.hasCleanupFailure -or -not $s.hasReaderFailure) { throw 'Failure evidence lost' }
$originalPhases=$raw.Phases
foreach ($badProperty in @('Frequency','StartCallEnteredTicks')) {
 $bad=[pscustomobject]@{}
 foreach ($property in $originalPhases.PSObject.Properties) { $bad | Add-Member NoteProperty $property.Name $property.Value }
 $bad.$badProperty='SECRET private path';$raw.Phases=$bad
 try {
  $null=ConvertTo-CovenQuerySummary -Id 'kernel-file-schema' -Result $raw
  throw 'Invalid phase value accepted'
 } catch {
  if ($_.Exception.Message -cne 'Invalid query phase observations') { throw 'Phase validation lost fixed failure class' }
 } finally { $raw.Phases=$originalPhases }
}
$bad=[pscustomobject]@{}
foreach ($property in $originalPhases.PSObject.Properties) {
 if ($property.Name -cne 'FallbackReaderWaitReturnedTicks') { $bad | Add-Member NoteProperty $property.Name $property.Value }
}
$raw.Phases=$bad
try {
 $null=ConvertTo-CovenQuerySummary -Id 'kernel-file-schema' -Result $raw
 throw 'Absent nullable field accepted'
} catch {
 if ($_.Exception.Message -cne 'Invalid query phase observations') { throw 'Missing property lost fixed failure class' }
} finally { $raw.Phases=$originalPhases }
try { ConvertTo-CovenQuerySummary -Id 'SECRET' -Result $raw; throw 'Unexpected success' } catch { if ($_.Exception.Message -eq 'Unexpected success') { throw } }
# The fixture observation must distinguish each failed assertion member without
# publishing raw failure text, output, arbitrary fault labels or query identity.
if (-not (Get-Command ConvertTo-CovenCleanupObservation -ErrorAction SilentlyContinue)) { throw 'Cleanup observation projection missing' }
foreach ($case in @('empty-failure','root-not-exited','readers-unsettled')) {
 $raw.Failure=if ($case -eq 'empty-failure') { '' } else { 'SECRET failure' }
 $raw.RootExited=$case -ne 'root-not-exited'; $raw.ReadersSettled=$case -ne 'readers-unsettled'
 foreach ($fault in @('after-start','reader','SECRET private fault')) {
  $o=ConvertTo-CovenCleanupObservation -Result $raw -Fault $fault
  $expectedFault=if ($fault -in @('after-start','reader')) { $fault } else { 'other' }
  if ($o.fault -cne $expectedFault -or $o.projectionFailed -or $o.kind -cne 'cleanup-fixture') { throw 'Cleanup fixture identity lost' }
  if ($o.hasFailure -ne ($case -ne 'empty-failure') -or $o.rootExited -ne $raw.RootExited -or $o.readersSettled -ne $raw.ReadersSettled) { throw 'Cleanup predicate observations conflated' }
  if ($null -ne $o.stdoutHandleClosed -or $null -ne $o.stderrHandleClosed) { throw 'Unknown handle state was invented' }
  if ($o.phases.PSObject.Properties.Name.Count -ne 22 -or $o.phases.startCallReturnedTicks -ne 2L) { throw 'Cleanup phase projection lost' }
  $j=$o | ConvertTo-Json -Depth 5 -Compress
  if ($j -match 'SECRET|/Users|foreign-session|S-1-5-21|OperationEnd|xperf|Sha256|stdoutChars|stderrChars|"id"') { throw 'Cleanup fixture leaked private or unrelated query data' }
 }
}
$handle=[Microsoft.Win32.SafeHandles.SafeFileHandle]::new([IntPtr](-1),$false)
try {
 $o=ConvertTo-CovenCleanupObservation -Result $raw -Fault 'reader' -StdoutHandle $handle
 if ($o.stdoutHandleClosed -cne $false) { throw 'Open retained handle state lost' }
 $handle.Dispose()
 $o=ConvertTo-CovenCleanupObservation -Result $raw -Fault 'reader' -StdoutHandle $handle
 if ($o.stdoutHandleClosed -cne $true) { throw 'Closed retained handle state lost' }
} finally { $handle.Dispose() }
$raw.Phases=$bad
$o=ConvertTo-CovenCleanupObservation -Result $raw -Fault 'SECRET private fault'
$j=$o | ConvertTo-Json -Compress
if (-not $o.projectionFailed -or $o.fault -cne 'other' -or $o.PSObject.Properties.Name.Count -ne 3 -or $j -match 'SECRET') { throw 'Projection failure lost its fixed safe marker' }
'query summary privacy and accounting PASS'
