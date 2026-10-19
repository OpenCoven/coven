$ErrorActionPreference='Stop'
. (Join-Path $PSScriptRoot 'token-binding.ps1')
$root=Join-Path ([IO.Path]::GetTempPath()) ('coven-token-runtime-controlled-'+[Guid]::NewGuid().ToString('N'))
[IO.Directory]::CreateDirectory($root) | Out-Null
foreach($name in @('advapi32.dll','kernel32.dll')){[IO.File]::WriteAllText((Join-Path $root $name),'controlled library')}
$script:modules=[Collections.Generic.List[object]]::new()
$script:leases=@{};$script:loads=0;$script:freed=[Collections.Generic.List[int]]::new();$script:failLoad=0;$script:failFree=0
function Get-CovenTokenLoadedModuleSnapshot {return ,$script:modules.ToArray()}
function Add-CovenTokenNativeReference([string]$Path){
    $script:loads++
    if($script:loads -eq $script:failLoad){throw 'PRIVATE_LOAD_SENTINEL'}
    if(@($script:modules | Where-Object {$_.FileName -ceq $Path}).Count -eq 0){$script:modules.Add([pscustomobject]@{ModuleName=[IO.Path]::GetFileName($Path);FileName=$Path})}
    $script:leases[$script:loads]=$Path
    return [IntPtr]::new($script:loads)
}
function Remove-CovenTokenNativeReference([IntPtr]$Handle){
    $value=$Handle.ToInt32();$script:freed.Add($value)
    if(-not $script:leases.ContainsKey($value)){throw 'Foreign reference released'}
    if($value -eq $script:failFree){throw 'PRIVATE_RELEASE_SENTINEL'}
    $script:leases.Remove($value)
}
function Reset-Controlled {
    $script:modules.Clear();$script:leases=@{};$script:loads=0;$script:freed.Clear();$script:failLoad=0;$script:failFree=0
}
function Require-Controlled([bool]$Value){if(-not $Value){throw 'Runtime controlled assertion failed'}}
$cases=0
try {
    $managed=Get-CovenTokenManagedCapture
    $binding=Open-CovenTokenRuntimeReferences -Managed $managed -SystemDirectory $root
    Require-Controlled ((Test-CovenTokenRuntimeBinding $binding) -and $script:loads -eq 2)
    Close-CovenTokenRuntimeBinding $binding
    Require-Controlled ($binding.Closed -and -not $binding.ReleaseFailed -and ($script:freed -join ',') -ceq '2,1')
    # Controlled loaded-module observations deliberately survive owned releases.
    Require-Controlled ($script:modules.Count -eq 2);$cases++
    try{Close-CovenTokenRuntimeBinding $binding;throw 'Double release admitted'}catch{if($_.Exception.Message -eq 'Double release admitted'){throw}}
    Require-Controlled ($script:freed.Count -eq 2);$cases++
    Reset-Controlled
    $script:modules.Add([pscustomobject]@{ModuleName='advapi32.dll';FileName=(Join-Path $root 'foreign.dll')})
    try{Open-CovenTokenRuntimeReferences -Managed $managed -SystemDirectory $root | Out-Null;throw 'Foreign preload admitted'}catch{if($_.Exception.Message -eq 'Foreign preload admitted'){throw};Require-Controlled (-not $_.Exception.Data['TokenReleaseAttempted'])}
    Require-Controlled ($script:loads -eq 0 -and $script:freed.Count -eq 0);$cases++
    Reset-Controlled;$script:failLoad=2
    try{Open-CovenTokenRuntimeReferences -Managed $managed -SystemDirectory $root | Out-Null;throw 'Failed pin admitted'}catch{if($_.Exception.Message -eq 'Failed pin admitted'){throw};Require-Controlled ($_.Exception.Data['TokenReleaseAttempted'] -and -not $_.Exception.Data['TokenReleaseFailed'] -and $_.Exception.Data['TokenBindingStage'] -ceq 'reference')}
    Require-Controlled (($script:freed -join ',') -ceq '1');$cases++
    Reset-Controlled;$script:failLoad=2;$script:failFree=1
    try{Open-CovenTokenRuntimeReferences -Managed $managed -SystemDirectory $root | Out-Null;throw 'Failed cleanup admitted'}catch{if($_.Exception.Message -eq 'Failed cleanup admitted'){throw};Require-Controlled ($_.Exception.Message -ceq 'Token runtime binding failed' -and $_.Exception.Data['TokenReleaseFailed'] -and $_.Exception.Data['TokenBindingStage'] -ceq 'reference')}
    Require-Controlled (($script:freed -join ',') -ceq '1');$cases++
    Reset-Controlled
    $binding=Open-CovenTokenRuntimeReferences -Managed $managed -SystemDirectory $root
    $script:failFree=2
    try{Close-CovenTokenRuntimeBinding $binding;throw 'Failed release admitted'}catch{if($_.Exception.Message -eq 'Failed release admitted'){throw}}
    Require-Controlled (($script:freed -join ',') -ceq '2,1' -and $binding.ReleaseFailed -and -not $binding.Closed);$cases++
    $before=$script:freed.Count
    try{Close-CovenTokenRuntimeBinding $binding;throw 'Failed release retried'}catch{if($_.Exception.Message -eq 'Failed release retried'){throw}}
    Require-Controlled ($script:freed.Count -eq $before);$cases++
    Reset-Controlled
    $binding=Open-CovenTokenRuntimeReferences -Managed $managed -SystemDirectory $root
    $libraryPath=$binding.Libraries[0].Path;$original=[IO.File]::ReadAllBytes($libraryPath)
    [IO.File]::WriteAllText($libraryPath,'changed controlled library')
    Require-Controlled (-not (Test-CovenTokenRuntimeBinding $binding));[IO.File]::WriteAllBytes($libraryPath,$original)
    Close-CovenTokenRuntimeBinding $binding;$cases++
    Reset-Controlled
    $binding=Open-CovenTokenRuntimeReferences -Managed $managed -SystemDirectory $root
    $originalHash=$managed.Files[0].Sha256;$managed.Files[0].Sha256='a'*64
    Require-Controlled (-not (Test-CovenTokenRuntimeBinding $binding));$managed.Files[0].Sha256=$originalHash
    Close-CovenTokenRuntimeBinding $binding;$cases++
    Reset-Controlled
    $binding=Open-CovenTokenRuntimeReferences -Managed $managed -SystemDirectory $root
    $script:modules[0].FileName=Join-Path $root 'foreign.dll'
    Require-Controlled (-not (Test-CovenTokenRuntimeBinding $binding))
    Close-CovenTokenRuntimeBinding $binding;Require-Controlled (($script:freed -join ',') -ceq '2,1');$cases++
    $path=Get-CovenTokenSystemLibraryPath -SystemDirectory $root -LibraryName 'kernel32.dll'
    $duplicate=@([pscustomobject]@{ModuleName='kernel32.dll';FileName=$path},[pscustomobject]@{ModuleName='kernel32.dll';FileName=$path})
    Require-Controlled (-not (Test-CovenTokenLoadedLibrary -ExpectedPath $path -LibraryName 'kernel32.dll' -Modules $duplicate -Required $true));$cases++
} finally {Remove-Item -LiteralPath $root -Recurse -Force}
"token runtime binding controlled PASS cases=$cases (fake native reference calls only)"
