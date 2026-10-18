$ErrorActionPreference = 'Stop'
. "$PSScriptRoot/query-summary.ps1"
$raw = [pscustomobject]@{ Started=$true; TimedOut=$false; OutputLimited=$false; Reaped=$true; RootExited=$true; ReadersSettled=$true; ExitCode=0; ElapsedMs=12; Stdout='SECRET /Users/private foreign-session S-1-5-21-999 Flush OperationEnd'; Stderr='SECRET'; Failure='SECRET'; CleanupFailure='SECRET'; ReaderFailure='SECRET' }
$s = ConvertTo-CovenQuerySummary -Id 'kernel-file-schema' -Result $raw
$j = $s | ConvertTo-Json -Depth 5 -Compress
if ($j -match 'SECRET|/Users|foreign-session|S-1-5-21|OperationEnd') { throw 'Raw query data leaked' }
if ($s.stdoutChars -ne $raw.Stdout.Length -or $s.stderrChars -ne 6) { throw 'Missing output accounting' }
if ($s.stdoutSha256 -notmatch '^[0-9a-f]{64}$') { throw 'Missing output digest' }
if (-not $s.hasFailure -or -not $s.hasCleanupFailure -or -not $s.hasReaderFailure) { throw 'Failure evidence lost' }
try { ConvertTo-CovenQuerySummary -Id 'SECRET' -Result $raw; throw 'Unexpected success' } catch { if ($_.Exception.Message -eq 'Unexpected success') { throw } }
'query summary privacy and accounting PASS'
