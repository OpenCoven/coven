$ErrorActionPreference='Stop'
. "$PSScriptRoot/tool-identity.ps1"
. "$PSScriptRoot/tool-discovery.ps1"
$t=Join-Path ([IO.Path]::GetTempPath()) ('coven-discovery-'+[guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $t | Out-Null
try {
 foreach ($dir in @('path','wpt','system32')) { New-Item -ItemType Directory -Path (Join-Path $t $dir) | Out-Null }
 [IO.File]::WriteAllText((Join-Path $t 'path/xperf.exe'),'path fixture')
 [IO.File]::WriteAllText((Join-Path $t 'wpt/xperf.exe'),'wpt fixture')
 [IO.File]::WriteAllText((Join-Path $t 'wpt/tracelog.exe'),'wpt fixture')
 [IO.File]::WriteAllText((Join-Path $t 'system32/wevtutil.exe'),'system fixture')
 $lookup={ param($name) if ($name -eq 'xperf.exe') { Join-Path $t 'path/xperf.exe' } }
 $v=Get-CovenInstalledToolIdentities -PathLookup $lookup -WptDirectory (Join-Path $t 'wpt') -SystemDirectory (Join-Path $t 'system32')
 if ($v.Count -ne 6 -or $v.xperf.summary.locationKind -ne 'path' -or $v.tracelog.summary.locationKind -ne 'wpt') { throw 'Wrong discovery preference' }
 if ($v.wevtutil.summary.locationKind -ne 'system32' -or -not $v.wevtutil.summary.present) { throw 'Wrong system tool' }
 if ($v.WPAExporter.summary.present -or $null -ne $v.WPAExporter.path) { throw 'Missing tool invented' }
 if ($v.WPAExporter.summary.PSObject.Properties.Name -notcontains 'admitted') { throw 'Missing candidate lacks uniform admission evidence' }
 $none=Get-CovenInstalledToolIdentities -PathLookup { param($name) $null } -WptDirectory '' -SystemDirectory ''
 if ($none.xperf.summary.PSObject.Properties.Name -notcontains 'admitted') { throw 'Absent discovery root lacks uniform admission evidence' }
 'tool discovery PASS'
} finally { Remove-Item -LiteralPath $t -Recurse -Force }
