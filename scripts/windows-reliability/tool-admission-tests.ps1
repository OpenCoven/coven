$ErrorActionPreference='Stop'
. "$PSScriptRoot/tool-identity.ps1"
$t=[IO.Directory]::CreateTempSubdirectory('coven-shadow-tool-').FullName
try {
 $p=Join-Path $t 'xperf.exe';[IO.File]::WriteAllText($p,'unsigned shadow fixture')
 $v=Get-CovenToolIdentity -Tool xperf -AbsolutePath $p -LocationKind path
 if ($null -ne $v.path -or $v.summary.admitted) { throw 'Unsigned shadow tool admitted' }
 if (-not $v.summary.present -or $v.summary.sha256 -notmatch '^[a-f0-9]{64}$') { throw 'Rejected candidate evidence lost' }
 foreach ($status in @('NotSigned','HashMismatch','NotTrusted','unavailable')) {
  if (Test-CovenToolAdmission -Tool xperf -SignatureStatus $status -MicrosoftSigner $true -OriginalFilename xperf.exe) { throw 'Invalid signature admitted' }
 }
 if (Test-CovenToolAdmission -Tool xperf -SignatureStatus Valid -MicrosoftSigner $false -OriginalFilename xperf.exe) { throw 'Other signer admitted' }
 if (Test-CovenToolAdmission -Tool xperf -SignatureStatus Valid -MicrosoftSigner $true -OriginalFilename other.exe) { throw 'Renamed signed tool admitted' }
 if (-not (Test-CovenToolAdmission -Tool xperf -SignatureStatus Valid -MicrosoftSigner $true -OriginalFilename XPerf.exe)) { throw 'Trusted metadata policy failed' }
 'tool admission policy PASS (synthetic metadata; no native signature validation)'
} finally { Remove-Item -LiteralPath $t -Recurse -Force }
