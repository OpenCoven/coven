$ErrorActionPreference='Stop'
$parts=foreach ($name in @('token-privileges.cs','token-privileges-controls.cs')) {
    $path=Join-Path $PSScriptRoot $name
    if (-not [IO.File]::Exists($path)) { throw 'Token privilege controlled source missing' }
    "namespace CovenTokenControlled {`n"+[IO.File]::ReadAllText($path,[Text.UTF8Encoding]::new($false,$true))+"`n}"
}
$references=@(Get-ChildItem -LiteralPath (Join-Path $PSHOME 'ref') -Filter '*.dll' | ForEach-Object FullName)
Add-Type -TypeDefinition ($parts -join "`n") -ReferencedAssemblies $references -ErrorAction Stop
$report=[CovenTokenControlled.CovenTokenPrivilegeControls]::Run()
if ($report.Failures.Length -ne 0) { throw ('Token privilege controlled failures: '+($report.Failures -join ',')) }
"token privilege controlled PASS cases=$($report.Cases) (no native API invocation)"
