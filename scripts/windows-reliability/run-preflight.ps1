$ErrorActionPreference='Stop'
if (-not [OperatingSystem]::IsWindows()) { throw 'Windows host required' }
$files=@('query-helper.ps1','query-plan.ps1','query-summary.ps1','query-executor.ps1','tool-identity.ps1','tool-discovery.ps1','provider-schema.ps1','preflight-core.ps1','receipt-writer.ps1','source-stability.ps1')
# Capture before ANY helper is loaded or query is run.
$sources=@(@('run-preflight.ps1') + $files | ForEach-Object {
    [pscustomobject]@{file=$_;sha256=(Get-FileHash -LiteralPath (Join-Path $PSScriptRoot $_) -Algorithm SHA256).Hash.ToLowerInvariant()}
})
foreach ($file in $files) { . (Join-Path $PSScriptRoot $file) }
$result=Invoke-CovenNativeMetadataPreflight
$stable=$true
try { Assert-CovenSourceStable -Before $sources -Directory $PSScriptRoot } catch { $stable=$false }
$result.public | Add-Member -NotePropertyName sourceFiles -NotePropertyValue @($sources)
$result.public | Add-Member -NotePropertyName sourceStable -NotePropertyValue $stable
$paths=Write-CovenPreflightReceipt -Result $result
# Private raw results remain on the disposable runner. No raw stdout/stderr in logs or artifacts.
# A workflow must upload this single public file explicitly, never its parent run directory.
if ($env:GITHUB_OUTPUT) {
    [IO.File]::AppendAllText($env:GITHUB_OUTPUT,"public-receipt=$($paths.publicDirectory)/receipt.json`n",[Text.UTF8Encoding]::new($false))
}
Write-Output 'Read-only metadata queries finished; public receipt written; no trace capture attempted.'
if ($result.public.aborted -or -not $stable) { exit 1 }
