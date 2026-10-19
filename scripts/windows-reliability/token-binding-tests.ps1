$ErrorActionPreference='Stop'
. (Join-Path $PSScriptRoot 'token-binding.ps1')
$script:cases=0
function Assert-Controlled([bool]$Value){if(-not $Value){throw "Controlled assertion failed at case=$script:cases"}}
function Reject-Controlled([scriptblock]$Action){try{& $Action | Out-Null}catch{return};throw "Invalid source admitted at case=$script:cases"}
$root=Join-Path ([IO.Path]::GetTempPath()) ('coven-token-binding-controlled-'+[Guid]::NewGuid().ToString('N'))
$inventory=Get-CovenTokenSourceInventory
[IO.Directory]::CreateDirectory($root) | Out-Null
try {
    foreach($relative in $inventory.Values){
        $path=Join-Path $root $relative
        [IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($path)) | Out-Null
        [IO.File]::WriteAllText($path,'controlled source',[Text.UTF8Encoding]::new($false))
    }
    $capture=Get-CovenTokenSourceCapture -Directory $root -Commit ('a'*40)
    Assert-Controlled ($capture.Files.Count -eq $inventory.Count -and (Test-CovenTokenSourceCapture $capture));$script:cases++
    $savedProcessDirectory=[Environment]::CurrentDirectory
    try {
        [Environment]::CurrentDirectory=$root
        $relativeRoot='.'
        Assert-Controlled (-not [IO.Path]::IsPathFullyQualified($relativeRoot))
        # The relative input must name an existing complete fixture: rejecting a
        # nonexistent path would not discriminate the absolute-root guard.
        $relativeAbsolute=[IO.Path]::GetFullPath($relativeRoot)
        $relativeCapture=Get-CovenTokenSourceCapture -Directory $relativeAbsolute -Commit ('a'*40)
        Assert-Controlled ($relativeCapture.Files.Count -eq $inventory.Count -and (Test-CovenTokenSourceCapture $relativeCapture))
        Reject-Controlled {Get-CovenTokenSourceCapture -Directory $relativeRoot -Commit ('a'*40)};$script:cases++
    } finally {[Environment]::CurrentDirectory=$savedProcessDirectory}
    # GetRelativePath deliberately returns the absolute target across volumes.
    # Verify that native Windows contrast without using it as a negative input.
    if ($IsWindows -and -not [StringComparer]::OrdinalIgnoreCase.Equals(
        [IO.Path]::GetPathRoot((Get-Location).Path),[IO.Path]::GetPathRoot($root))) {
        $crossVolume=[IO.Path]::GetRelativePath((Get-Location).Path,$root)
        Assert-Controlled ([IO.Path]::IsPathFullyQualified($crossVolume))
        $crossCapture=Get-CovenTokenSourceCapture -Directory $crossVolume -Commit ('a'*40)
        Assert-Controlled ($crossCapture.Files.Count -eq $inventory.Count -and (Test-CovenTokenSourceCapture $crossCapture));$script:cases++
        'token source binding cross-volume fixture precondition PASS'
    }
    $rootFile=$root+'-file'
    try {
        [IO.File]::WriteAllText($rootFile,'controlled root file')
        Reject-Controlled {Get-CovenTokenSourceCapture -Directory $rootFile -Commit ('a'*40)};$script:cases++
    } finally {[IO.File]::Delete($rootFile)}
    # Own only the link entry; never recurse through it during fixture cleanup.
    $alias=$root+'-alias'
    try {
        New-Item -ItemType SymbolicLink -Path $alias -Target $root | Out-Null
        Reject-Controlled {Get-CovenTokenSourceCapture -Directory $alias -Commit ('a'*40)};$script:cases++
    } finally {if([IO.File]::Exists($alias) -or [IO.Directory]::Exists($alias)){[IO.Directory]::Delete($alias)}}
    $savedRoot=$root+'-original'
    [IO.Directory]::Move($root,$savedRoot)
    try {
        New-Item -ItemType SymbolicLink -Path $root -Target $savedRoot | Out-Null
        Assert-Controlled (-not (Test-CovenTokenSourceCapture $capture));$script:cases++
    } finally {
        if([IO.Directory]::Exists($root)){[IO.Directory]::Delete($root)}
        [IO.Directory]::Move($savedRoot,$root)
    }
    Assert-Controlled (Test-CovenTokenSourceCapture $capture)
    $file=$capture.Files[0];$original=[IO.File]::ReadAllBytes($file.Path)
    [IO.File]::WriteAllText($file.Path,'changed source')
    Assert-Controlled (-not (Test-CovenTokenSourceCapture $capture));[IO.File]::WriteAllBytes($file.Path,$original);$script:cases++
    $file.Bytes[0]=$file.Bytes[0] -bxor 1
    Assert-Controlled (-not (Test-CovenTokenSourceCapture $capture));$file.Bytes[0]=$file.Bytes[0] -bxor 1;$script:cases++
    $id=$file.Id;$file.Id=$id.ToUpperInvariant()
    Assert-Controlled (-not (Test-CovenTokenSourceCapture $capture));$file.Id=$id;$script:cases++
    $secondId=$capture.Files[1].Id;$capture.Files[1].Id=$id
    Assert-Controlled (-not (Test-CovenTokenSourceCapture $capture));$capture.Files[1].Id=$secondId;$script:cases++
    [IO.File]::WriteAllBytes($file.Path,[byte[]]::new(262145))
    Reject-Controlled {Get-CovenTokenSourceCapture -Directory $root -Commit ('a'*40)}
    [IO.File]::WriteAllBytes($file.Path,$original);$script:cases++
    foreach($relative in $inventory.Values){[IO.File]::WriteAllBytes((Join-Path $root $relative),[byte[]]::new(131072))}
    Reject-Controlled {Get-CovenTokenSourceCapture -Directory $root -Commit ('a'*40)};$script:cases++
    foreach($relative in $inventory.Values){[IO.File]::WriteAllText((Join-Path $root $relative),'controlled source')}
    Reject-Controlled {Get-CovenTokenSourceCapture -Directory $root -Commit 'not-a-commit'};$script:cases++
    $candidate=Join-Path $root $inventory.Values[0]
    $target=Join-Path $root 'controlled-target'
    [IO.File]::WriteAllText($target,'controlled target')
    [IO.File]::Delete($candidate)
    New-Item -ItemType SymbolicLink -Path $candidate -Target $target | Out-Null
    Reject-Controlled {Get-CovenTokenSourceCapture -Directory $root -Commit ('a'*40)};$script:cases++
    [IO.File]::Delete($candidate)
    New-Item -ItemType HardLink -Path $candidate -Target $target | Out-Null
    Reject-Controlled {Get-CovenTokenSourceCapture -Directory $root -Commit ('a'*40)};$script:cases++
    [IO.File]::Delete($candidate);[IO.File]::WriteAllText($candidate,'controlled source')
    # Only the five C# units are copied from this checkout. The surrounding
    # synthetic tree is not a Git/native admission fixture.
    foreach($id in @('privilege-projector','reader','native-adapter','public-encoder','binding-types')){
        $relative=$inventory[$id]
        [IO.File]::WriteAllBytes((Join-Path $root $relative),[IO.File]::ReadAllBytes((Join-Path $PSScriptRoot ([IO.Path]::GetFileName($relative)))))
    }
    $compiledCapture=Get-CovenTokenSourceCapture -Directory $root -Commit ('a'*40)
    Add-CovenTokenCapturedTypes -Capture $compiledCapture
    $sourceDto=ConvertTo-CovenTokenSourceDto -Capture $compiledCapture
    $sourceObject=ConvertTo-CovenTokenPublicIdentity -Value $sourceDto
    Assert-Controlled ($sourceObject.Contains('beforeAfterMatch') -and $sourceObject.beforeAfterMatch -and $sourceObject.files.Count -eq $inventory.Count -and $sourceObject.files[0].id -ceq 'binding' -and $sourceObject.files[-1].id -ceq 'workflow-controls');$script:cases++
    Reject-Controlled {ConvertTo-CovenTokenPublicIdentity -Value $compiledCapture};$script:cases++
    Reject-Controlled {Add-CovenTokenCapturedTypes -Capture $compiledCapture};$script:cases++
} finally {Remove-Item -LiteralPath $root -Recurse -Force}
"token source binding controlled PASS cases=$script:cases (synthetic source tree/captured C# compilation; no collector/native invocation)"
