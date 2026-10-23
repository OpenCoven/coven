$ErrorActionPreference='Stop'
$parts=foreach ($name in @('token-privileges.cs','token-reader.cs','token-reader-controls.cs')) {
    $path=Join-Path $PSScriptRoot $name
    if (-not [IO.File]::Exists($path)) { throw 'Token reader controlled source missing' }
    "namespace CovenTokenReaderControlled {`n"+[IO.File]::ReadAllText($path,[Text.UTF8Encoding]::new($false,$true))+"`n}"
}
$references=@(Get-ChildItem -LiteralPath (Join-Path $PSHOME 'ref') -Filter '*.dll' | ForEach-Object FullName)
Add-Type -TypeDefinition ($parts -join "`n") -ReferencedAssemblies $references -ErrorAction Stop
$report=[CovenTokenReaderControlled.CovenTokenReaderControls]::Run()
if ($report.Failures.Length -ne 0) { throw ('Token reader controlled failures: '+($report.Failures -join ',')) }
"token reader controlled PASS cases=$($report.Cases) (no native API invocation)"
