$ErrorActionPreference='Stop'
. "$PSScriptRoot/tool-identity.ps1"
$t=[IO.Directory]::CreateTempSubdirectory('coven-shadow-tool-').FullName
try {
 $p=Join-Path $t 'xperf.exe';[IO.File]::WriteAllText($p,'unsigned shadow fixture')
 $v=Get-CovenToolIdentity -Tool xperf -AbsolutePath $p -LocationKind path
 if ($null -ne $v.path -or $v.summary.admitted) { throw 'Unsigned shadow tool admitted' }
 if (-not $v.summary.present -or $v.summary.sha256 -notmatch '^[a-f0-9]{64}$') { throw 'Rejected candidate evidence lost' }
 if ($v.summary.originalFilenameClass -notin @('missing','empty') -or $v.summary.stableHashMatched -ne $true) { throw 'Unsigned resource observations lost' }
 foreach ($status in @('NotSigned','HashMismatch','NotTrusted','unavailable')) {
  if (Test-CovenToolAdmission -Tool xperf -SignatureStatus $status -MicrosoftSigner $true -OriginalFilename xperf.exe) { throw 'Invalid signature admitted' }
 }
 if (Test-CovenToolAdmission -Tool xperf -SignatureStatus Valid -MicrosoftSigner $false -OriginalFilename xperf.exe) { throw 'Other signer admitted' }
 if (Test-CovenToolAdmission -Tool xperf -SignatureStatus Valid -MicrosoftSigner $true -OriginalFilename other.exe) { throw 'Renamed signed tool admitted' }
 if (-not (Test-CovenToolAdmission -Tool xperf -SignatureStatus Valid -MicrosoftSigner $true -OriginalFilename XPerf.exe)) { throw 'Trusted metadata policy failed' }
 $cases=@(
  @{value=$null;expected='missing'},@{value='';expected='empty'},
  @{value='xperf.exe';expected='expected-exe'},@{value='XPerf.EXE';expected='expected-exe'},
  @{value='xperf';expected='expected-bare'},@{value='xperf.dll';expected='expected-dll'},
  @{value='xperf.exe.mui';expected='expected-exe-mui'},@{value='whoami.exe';expected='other'},
  @{value=' xperf.exe';expected='other'},@{value='xperf.exe ';expected='other'},
  @{value="xperf.exe`n";expected='other'},@{value="xperf.exe`0";expected='other'},
  @{value='C:\PRIVATE_SENTINEL\xperf.exe';expected='other'},
  @{value='/PRIVATE_SENTINEL/xperf.exe';expected='other'},
  @{value=('PRIVATE_SENTINEL'*1024);expected='other'}
 )
 foreach ($case in $cases) {
  $class=Get-CovenOriginalFilenameClass -Tool xperf -OriginalFilename $case.value
  if ($class -cne $case.expected -or $class -match 'PRIVATE_SENTINEL|xperf|whoami|[\\/]') { throw 'Filename class or privacy contract failed' }
 }
 # Keep this diagnostic independent from the existing linguistic comparison.
 # Its result can differ across .NET globalization backends; do not normalize
 # controls into an ordinal match or silently tighten invocation here.
 $control="xperf.exe`0"
 if ((Test-CovenToolAdmission -Tool xperf -SignatureStatus Valid -MicrosoftSigner $true -OriginalFilename $control) -ne ($control -ieq 'xperf.exe')) { throw 'Existing control comparison changed' }
 foreach ($tool in @('xperf','tracelog','tracerpt','WPAExporter','wevtutil','whoami')) {
  if ((Get-CovenOriginalFilenameClass -Tool $tool -OriginalFilename "$tool.exe") -cne 'expected-exe') { throw 'Supported tool classification failed' }
 }
 & {
  param($Path)
  # Controlled metadata/hash/signature outcomes, not native trust validation.
  $state=@{filename='xperf.exe';hashCalls=0;mutate=$false;failHash=0}
  function Get-Item { [pscustomobject]@{FullName=$Path;VersionInfo=[pscustomobject]@{FileMajorPart=1;FileMinorPart=2;FileBuildPart=3;FilePrivatePart=4;OriginalFilename=$state.filename}} }
  function Get-FileHash {
   $state.hashCalls++
   if ($state.hashCalls -eq $state.failHash) { throw 'Controlled hash failure' }
   $hash=if ($state.mutate -and $state.hashCalls -eq 2) { 'b'*64 } else { 'a'*64 }
   [pscustomobject]@{Hash=$hash}
  }
  function Get-AuthenticodeSignature { [pscustomobject]@{Status='Valid';SignerCertificate=[pscustomobject]@{SubjectName=$null}} }
  function Test-CovenMicrosoftSigner { $true }
  $trusted=Get-CovenToolIdentity -Tool xperf -AbsolutePath $Path -LocationKind path
  if (-not $trusted.summary.admitted -or $trusted.summary.originalFilenameClass -cne 'expected-exe' -or $trusted.summary.stableHashMatched -ne $true -or $trusted.path -ne $Path) { throw 'Controlled trusted projection failed' }
  $state.hashCalls=0;$state.mutate=$true
  $changed=Get-CovenToolIdentity -Tool xperf -AbsolutePath $Path -LocationKind path
  if ($changed.summary.admitted -or $null -ne $changed.path -or $changed.summary.stableHashMatched -ne $false -or $changed.summary.originalFilenameClass -cne 'expected-exe') { throw 'Changed hash admitted or evidence lost' }
  $state.hashCalls=0;$state.failHash=2
  $partial=Get-CovenToolIdentity -Tool xperf -AbsolutePath $Path -LocationKind path
  if (-not $partial.summary.identityFailed -or $partial.summary.admitted -or $null -ne $partial.summary.stableHashMatched -or $partial.summary.originalFilenameClass -cne 'expected-exe') { throw 'Partial observation erased after hash failure' }
  $state.hashCalls=0;$state.failHash=1
  $early=Get-CovenToolIdentity -Tool xperf -AbsolutePath $Path -LocationKind path
  if (-not $early.summary.identityFailed -or $early.summary.originalFilenameClass -cne 'not-observed' -or $null -ne $early.summary.stableHashMatched) { throw 'Early failure invented resource observations' }
  $state.hashCalls=0;$state.failHash=0;$state.mutate=$false;$state.filename='PRIVATE_SENTINEL/path/xperf.exe'
  $private=Get-CovenToolIdentity -Tool xperf -AbsolutePath $Path -LocationKind path
  if ($private.summary.admitted -or $private.summary.originalFilenameClass -cne 'other' -or ($private.summary | ConvertTo-Json -Compress) -match 'PRIVATE_SENTINEL|/path/') { throw 'Rejected private resource leaked or admitted' }
 } $p
 'tool admission policy PASS (synthetic metadata; no native signature validation)'
} finally { Remove-Item -LiteralPath $t -Recurse -Force }
