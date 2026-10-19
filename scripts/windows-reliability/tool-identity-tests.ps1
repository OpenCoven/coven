$ErrorActionPreference='Stop'
. "$PSScriptRoot/tool-identity.ps1"
$t=Join-Path ([IO.Path]::GetTempPath()) ('coven-tool-'+[guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $t | Out-Null
try {
 $p=Join-Path $t 'xperf.exe'; [IO.File]::WriteAllText($p,'synthetic binary identity fixture SECRET')
 $v=Get-CovenToolIdentity -Tool xperf -AbsolutePath $p -LocationKind path
 if ($v.summary.sha256 -notmatch '^[a-f0-9]{64}$' -or -not $v.summary.present) { throw 'Missing binary identity' }
 if ($null -ne $v.path -or $v.summary.admitted) { throw 'Unsigned fixture admitted' }
 $j=$v.summary | ConvertTo-Json -Compress
 if ($j.Contains($t) -or $j.Contains('SECRET')) { throw 'Private identity leaked' }
 $missing=Get-CovenToolIdentity -Tool xperf -AbsolutePath (Join-Path $t 'absent/xperf.exe') -LocationKind wpt
 if ($missing.summary.present -or $null -ne $missing.path) { throw 'Missing tool admitted' }
 foreach ($bad in @('xperf.exe',(Join-Path $t 'whoami.exe'))) {
  try { Get-CovenToolIdentity -Tool xperf -AbsolutePath $bad -LocationKind path; throw 'Unexpected acceptance' } catch { if ($_.Exception.Message -eq 'Unexpected acceptance') { throw } }
 }
 'tool identity PASS'
} finally { Remove-Item -LiteralPath $t -Recurse -Force }
