param([string]$FixtureDirectory='')
$ErrorActionPreference='Stop'
$parts=foreach ($name in @('token-privileges.cs','token-reader.cs','token-public.cs','token-public-controls.cs')) {
    $path=Join-Path $PSScriptRoot $name
    if (-not [IO.File]::Exists($path)) { throw 'Token public controlled source missing' }
    "namespace CovenTokenPublicControlled {`n"+[IO.File]::ReadAllText($path,[Text.UTF8Encoding]::new($false,$true))+"`n}"
}
$references=@(Get-ChildItem -LiteralPath (Join-Path $PSHOME 'ref') -Filter '*.dll' | ForEach-Object FullName)
Add-Type -TypeDefinition ($parts -join "`n") -ReferencedAssemblies $references -ErrorAction Stop
$report=[CovenTokenPublicControlled.CovenTokenPublicControls]::Run()
if ($report.Failures.Length -ne 0) { throw ('Token public controlled failures: '+($report.Failures -join ',')) }
if ($FixtureDirectory) {
    if (-not [IO.Path]::IsPathFullyQualified($FixtureDirectory) -or (Test-Path -LiteralPath $FixtureDirectory)) { throw 'Controlled fixture directory must be fresh and absolute' }
    [IO.Directory]::CreateDirectory($FixtureDirectory) | Out-Null
    foreach ($name in @('success','primary-cleanup','output-limit','serialization-failed')) {
        $payload=[CovenTokenPublicControlled.CovenTokenPublicControls]::Fixture($name)
        $stream=[IO.File]::Open((Join-Path $FixtureDirectory ($name+'.json')),[IO.FileMode]::CreateNew,[IO.FileAccess]::Write,[IO.FileShare]::None)
        try { $stream.Write($payload,0,$payload.Length) } finally { $stream.Dispose() }
    }
}
"token public controlled PASS cases=$($report.Cases) (synthetic results only; no native API)"
