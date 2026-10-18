$ErrorActionPreference='Stop'
. "$PSScriptRoot/source-stability.ps1"
$t=[IO.Directory]::CreateTempSubdirectory('coven-source-proof-').FullName
try {
 $p=Join-Path $t 'fixture.ps1';[IO.File]::WriteAllText($p,'before')
 $before=@([pscustomobject]@{file='fixture.ps1';sha256=(Get-FileHash -LiteralPath $p -Algorithm SHA256).Hash.ToLowerInvariant()})
 Assert-CovenSourceStable -Before $before -Directory $t
 [IO.File]::WriteAllText($p,'after')
 try { Assert-CovenSourceStable -Before $before -Directory $t;throw 'Changed source accepted' } catch { if ($_.Exception.Message -ne 'Preflight source changed') { throw } }
 'source stability PASS'
} finally { Remove-Item -LiteralPath $t -Recurse -Force }
