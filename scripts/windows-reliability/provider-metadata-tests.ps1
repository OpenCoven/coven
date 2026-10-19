$ErrorActionPreference='Stop'
$reader=Join-Path $PSScriptRoot 'provider-metadata-reader.cs'
if (-not (Test-Path -LiteralPath $reader)) { throw 'Bounded provider reader missing' }
Add-Type -Path @((Join-Path $PSScriptRoot 'provider-template.cs'),$reader,(Join-Path $PSScriptRoot 'provider-reader-tests.cs'))
[ControlledKernelTests]::Execute()

$privateApi=[ControlledKernelApi]::new()
$privateApi.Fault='private'
$projection=[CovenKernelReader]::Collect($privateApi,[ControlledKernelBuffers]::new())
$json=$projection | ConvertTo-Json -Depth 12 -Compress
if ($json -match 'PRIVATE_SENTINEL|rawXml|/Users/private|S-1-5-21') { throw 'Private metadata escaped reader projection' }
'provider reader controlled privacy export PASS (no native API invocation)'
