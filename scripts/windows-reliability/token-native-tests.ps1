$ErrorActionPreference='Stop'
$parts=foreach ($name in @('token-privileges.cs','token-reader.cs','token-native.cs','token-native-controls.cs')) {
    $path=Join-Path $PSScriptRoot $name
    if (-not [IO.File]::Exists($path)) { throw 'Token native controlled source missing' }
    "namespace CovenTokenNativeControlled {`n"+[IO.File]::ReadAllText($path,[Text.UTF8Encoding]::new($false,$true))+"`n}"
}
$references=@(Get-ChildItem -LiteralPath (Join-Path $PSHOME 'ref') -Filter '*.dll' | ForEach-Object FullName)
Add-Type -TypeDefinition ($parts -join "`n") -ReferencedAssemblies $references -ErrorAction Stop
$report=[CovenTokenNativeControlled.CovenTokenNativeControls]::Run()
if ($report.Failures.Length -ne 0) { throw ('Token native controlled failures: '+($report.Failures -join ',')) }
"token native controlled PASS cases=$($report.Cases) (controlled calls/reflection only; no Windows token API)"
