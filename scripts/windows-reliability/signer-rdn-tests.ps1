using namespace System.Security.Cryptography.X509Certificates
$ErrorActionPreference='Stop'
. "$PSScriptRoot/tool-identity.ps1"
$t=[IO.Directory]::CreateTempSubdirectory('coven-signer-test-').FullName
try {
 $p=Join-Path $t 'xperf.exe';[IO.File]::WriteAllText($p,'fixture')
 foreach ($case in @(
  @('CN="Example, O=Microsoft Corporation, Division", O=Other Corporation',$false),
  @('CN=Microsoft Windows, O=Microsoft Corporation, C=US',$true),
  @('CN=Other, O=Microsoft Corporation, O=Other Corporation',$false)
 )) {
  $script:testDn=[X500DistinguishedName]::new($case[0])
  function Get-AuthenticodeSignature { param($LiteralPath) [pscustomobject]@{Status='Valid';SignerCertificate=[pscustomobject]@{Subject=$script:testDn.Name;SubjectName=$script:testDn}} }
  $v=Get-CovenToolIdentity -Tool xperf -AbsolutePath $p -LocationKind path
  if ($v.summary.microsoftSigner -ne $case[1]) { throw 'Organization RDN misclassified' }
 }
 'structured signer RDN PASS'
} finally {
 Remove-Item Function:Get-AuthenticodeSignature -ErrorAction SilentlyContinue
 Remove-Item -LiteralPath $t -Recurse -Force
}
