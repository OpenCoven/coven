using namespace System.Runtime.InteropServices
$ErrorActionPreference='Stop'
Add-Type -Path @((Join-Path $PSScriptRoot 'provider-template.cs'),(Join-Path $PSScriptRoot 'provider-metadata-reader.cs'))
# Reflection checks import metadata without calling any Windows function.
$native=[CovenKernelReader].Assembly.GetType('CovenWindowsKernelApi')
if ($null -eq $native) { throw 'Fixed Windows metadata API adapter missing' }
$imports=@($native.GetMethods([Reflection.BindingFlags]'NonPublic,Static') | Where-Object { $null -ne $_.GetCustomAttributes([DllImportAttribute],$false)[0] })
if ($imports.Count -ne 6) { throw 'Unexpected native API import count' }
foreach ($method in $imports) {
    $dll=$method.GetCustomAttributes([DllImportAttribute],$false)[0]
    $search=$method.GetCustomAttributes([DefaultDllImportSearchPathsAttribute],$false)[0]
    if ($dll.Value -cne 'wevtapi.dll' -or -not $dll.SetLastError -or -not $dll.ExactSpelling -or $search.Paths -ne [DllImportSearchPath]::System32) { throw 'Unbounded native library import' }
}
'provider reader privacy/native import metadata PASS (no native API invocation)'
