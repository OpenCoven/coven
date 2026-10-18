$ErrorActionPreference='Stop'
. "$PSScriptRoot/receipt-writer.ps1"
$r=[pscustomobject]@{public=[pscustomobject]@{captureAttempted=$false;queries=@()};rawResults=@{query=[pscustomobject]@{Stdout='SECRET private path';Stderr='SECRET SID'}}}
$p=Write-CovenPreflightReceipt -Result $r
try {
 $pub=[IO.File]::ReadAllText((Join-Path $p.publicDirectory 'receipt.json'))
 $raw=[IO.File]::ReadAllText((Join-Path $p.privateDirectory 'queries.json'))
 if ($pub -match 'SECRET|rawResults|privateDirectory' -or $raw -notmatch 'SECRET') { throw 'Receipt separation failed' }
 if ([IO.Path]::GetDirectoryName($p.publicDirectory) -ne $p.root -or [IO.Path]::GetDirectoryName($p.privateDirectory) -ne $p.root -or $p.publicDirectory -eq $p.privateDirectory) { throw 'Unsafe artifact paths' }
 if (@(Get-ChildItem -LiteralPath $p.publicDirectory -Recurse -File).Count -ne 1) { throw 'Unexpected public files' }
 'receipt writer separation PASS'
} finally { Remove-Item -LiteralPath $p.root -Recurse -Force }
