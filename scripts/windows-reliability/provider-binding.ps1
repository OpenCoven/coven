# Private source buffers/paths never become public DTO properties.
function Get-CovenProviderSourceInventory {
    [ordered]@{
        workflow='.github/workflows/windows-reliability.yml'
        runner='scripts/windows-reliability/run-provider-schema.ps1'
        probe='scripts/windows-reliability/provider-schema-probe.ps1'
        binding='scripts/windows-reliability/provider-binding.ps1'
        'binding-tests'='scripts/windows-reliability/provider-binding-tests.ps1'
        reader='scripts/windows-reliability/provider-metadata-reader.cs'
        template='scripts/windows-reliability/provider-template.cs'
        'public-encoder'='scripts/windows-reliability/provider-public.cs'
        'metadata-tests'='scripts/windows-reliability/provider-metadata-tests.ps1'
        'controlled-api'='scripts/windows-reliability/provider-reader-tests.cs'
        'template-tests'='scripts/windows-reliability/provider-template-tests.ps1'
        'adapter-tests'='scripts/windows-reliability/provider-adapter-tests.ps1'
        'encoder-tests'='scripts/windows-reliability/provider-public-tests.ps1'
        'controlled-encoder'='scripts/windows-reliability/provider-public-tests.cs'
        'query-helper'='scripts/windows-reliability/query-helper.ps1'
        'query-summary'='scripts/windows-reliability/query-summary.ps1'
        'source-stability'='scripts/windows-reliability/source-stability.ps1'
        'phase-validator'='scripts/windows-reliability/public_receipt.py'
        validator='scripts/windows-reliability/provider_schema_receipt.py'
        'validator-tests'='scripts/windows-reliability/provider_schema_receipt_tests.py'
        'envelope-tests'='scripts/windows-reliability/provider_envelope_tests.py'
    }
}
function Read-CovenProviderSourceBytes {
    param([Parameter(Mandatory)][string]$Path)
    $stream=$null
    $memory=[IO.MemoryStream]::new()
    try {
        $stream=[IO.File]::Open($Path,[IO.FileMode]::Open,[IO.FileAccess]::Read,[IO.FileShare]::Read)
        $buffer=[byte[]]::new(8192)
        while ($true) {
            $remaining=262144-$memory.Length
            $count=$stream.Read($buffer,0,[int][Math]::Min($buffer.Length,$remaining+1))
            if ($count -eq 0) { break }
            if ($count -gt $remaining) { throw 'Provider source size limit' }
            $memory.Write($buffer,0,$count)
        }
        return ,$memory.ToArray()
    } finally {
        if ($null -ne $stream) { $stream.Dispose() }
        $memory.Dispose()
    }
}
function Get-CovenProviderSourceHash {
    param([Parameter(Mandatory)][AllowEmptyCollection()][byte[]]$Bytes)
    [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($Bytes)).ToLowerInvariant()
}
function Assert-CovenProviderSourcePath {
    param([string]$Directory,[string]$Relative)
    $path=$Directory
    foreach ($part in $Relative.Split('/')) {
        $path=Join-Path $path $part
        $attributes=[IO.File]::GetAttributes($path)
        if (($attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) { throw 'Provider source reparse point' }
        $item=Get-Item -LiteralPath $path -Force -ErrorAction Stop
        if (-not [string]::IsNullOrEmpty($item.LinkType)) { throw 'Provider source link is not an owned plain path' }
    }
    if (($attributes -band [IO.FileAttributes]::Directory) -ne 0) { throw 'Provider source is not a file' }
    $path
}
function Get-CovenProviderSourceCapture {
    param([Parameter(Mandatory)][string]$Directory,[Parameter(Mandatory)][string]$Commit)
    try {
        if ($Commit -cnotmatch '^[0-9a-f]{40}$') { throw 'commit' }
        $root=[IO.Path]::TrimEndingDirectorySeparator([IO.Path]::GetFullPath($Directory))
        $inventory=Get-CovenProviderSourceInventory
        $files=[Collections.Generic.List[object]]::new()
        [long]$total=0
        foreach ($id in $inventory.Keys) {
            $path=Assert-CovenProviderSourcePath -Directory $root -Relative $inventory[$id]
            $bytes=Read-CovenProviderSourceBytes -Path $path
            $total+=$bytes.Length
            if ($total -gt 2097152) { throw 'total' }
            $files.Add([pscustomobject]@{Id=$id;Path=$path;Bytes=$bytes;Sha256=(Get-CovenProviderSourceHash -Bytes $bytes)})
        }
        [pscustomobject]@{Directory=$root;Commit=$Commit;Files=$files}
    } catch { throw 'Provider source capture failed' }
}
function Test-CovenProviderSourceCapture {
    param([Parameter(Mandatory)]$Capture)
    try {
        $inventory=Get-CovenProviderSourceInventory
        if ($Capture.Commit -isnot [string] -or $Capture.Commit -cnotmatch '^[0-9a-f]{40}$' -or $Capture.Files.Count -ne $inventory.Count) { return $false }
        $expectedIds=[Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
        foreach ($id in $inventory.Keys) { $expectedIds.Add([string]$id) | Out-Null }
        $seen=[Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
        [long]$total=0
        foreach ($file in $Capture.Files) {
            if (-not $expectedIds.Contains($file.Id) -or -not $seen.Add($file.Id) -or $file.Bytes -isnot [byte[]]) { return $false }
            $path=Assert-CovenProviderSourcePath -Directory $Capture.Directory -Relative $inventory[$file.Id]
            if ($path -cne $file.Path -or $file.Bytes.Length -gt 262144) { return $false }
            $total+=$file.Bytes.Length
            if ($total -gt 2097152) { return $false }
            if ((Get-CovenProviderSourceHash -Bytes $file.Bytes) -cne $file.Sha256) { return $false }
            if ((Get-CovenProviderSourceHash -Bytes (Read-CovenProviderSourceBytes -Path $path)) -cne $file.Sha256) { return $false }
        }
        return $seen.SetEquals($expectedIds)
    } catch { return $false }
}
function Add-CovenProviderCapturedTypes {
    param([Parameter(Mandatory)]$Capture)
    if (-not (Test-CovenProviderSourceCapture -Capture $Capture)) { throw 'Provider source changed before compilation' }
    if ('CovenProviderBound.CovenKernelTemplate' -as [type]) { throw 'Provider compiled type already present' }
    $utf8=[Text.UTF8Encoding]::new($false,$true)
    $parts=foreach ($id in @('template','reader','public-encoder')) {
        $file=@($Capture.Files | Where-Object { $_.Id -ceq $id })
        if ($file.Count -ne 1) { throw 'Provider compilation source missing' }
        # Keep each captured compilation unit's usings inside the same fixed namespace.
        "namespace CovenProviderBound {`n"+$utf8.GetString($file[0].Bytes)+"`n}"
    }
    $references=@(Get-ChildItem -LiteralPath (Join-Path $PSHOME 'ref') -Filter '*.dll' | ForEach-Object FullName)
    Add-Type -TypeDefinition ($parts -join "`n") -ReferencedAssemblies $references -ErrorAction Stop
    if (-not (Test-CovenProviderSourceCapture -Capture $Capture)) { throw 'Provider source changed during compilation' }
}

# Paths and reflection objects remain private. Only selected hashes/versions are projected.
function Get-CovenProviderManagedCapture {
    $types=[ordered]@{corelib=[object];powershell=[System.Management.Automation.PSObject];json=[Text.Json.JsonSerializer];xml=[Xml.XmlReader];crypto=[Security.Cryptography.SHA256]}
    $files=[Collections.Generic.List[object]]::new()
    $executable=[Environment]::ProcessPath
    $files.Add([pscustomobject]@{Kind='executable';Path=$executable;Sha256=(Get-FileHash -LiteralPath $executable -Algorithm SHA256 -ErrorAction Stop).Hash.ToLowerInvariant();Version=@()})
    foreach ($kind in $types.Keys) {
        $assembly=$types[$kind].Assembly
        $path=$assembly.Location
        if (-not [IO.Path]::IsPathFullyQualified($path)) { throw 'Provider runtime file unavailable' }
        $version=$assembly.GetName().Version
        $parts=@($version.Major,$version.Minor,$version.Build,$version.Revision)
        if (@($parts | Where-Object { $_ -lt 0 }).Count -ne 0) { throw 'Provider runtime version unavailable' }
        $files.Add([pscustomobject]@{Kind=$kind;Path=$path;Sha256=(Get-FileHash -LiteralPath $path -Algorithm SHA256 -ErrorAction Stop).Hash.ToLowerInvariant();Version=$parts})
    }
    [pscustomobject]@{Files=$files}
}
function Test-CovenProviderManagedCapture {
    param([Parameter(Mandatory)]$Capture)
    try {
        $current=Get-CovenProviderManagedCapture
        if ($Capture.Files.Count -ne $current.Files.Count) { return $false }
        for ($i=0;$i -lt $current.Files.Count;$i++) {
            $before=$Capture.Files[$i];$after=$current.Files[$i]
            if ($before.Kind -cne $after.Kind -or $before.Path -cne $after.Path -or $before.Sha256 -cne $after.Sha256 -or
                ($before.Version -join ',') -cne ($after.Version -join ',')) { return $false }
        }
        return $true
    } catch { return $false }
}
function Test-CovenProviderLoadedLibrary {
    param([Parameter(Mandatory)][string]$ExpectedPath,[AllowEmptyCollection()][object[]]$Modules,[bool]$Required)
    try {
        $matches=@($Modules | Where-Object { [string]::Equals($_.ModuleName,'wevtapi.dll',[StringComparison]::OrdinalIgnoreCase) })
        if ($matches.Count -eq 0) { return (-not $Required) }
        return $matches.Count -eq 1 -and [string]::Equals([IO.Path]::GetFullPath($matches[0].FileName),$ExpectedPath,[StringComparison]::OrdinalIgnoreCase)
    } catch { return $false }
}
function Assert-CovenProviderLoadedLibrary {
    param([string]$Path,[bool]$Required)
    $process=[Diagnostics.Process]::GetCurrentProcess()
    try {
        if (-not (Test-CovenProviderLoadedLibrary -ExpectedPath $Path -Modules @($process.Modules) -Required $Required)) { throw 'Provider native library identity mismatch' }
    } finally { $process.Dispose() }
}
function Open-CovenProviderRuntimeBinding {
    if (-not $IsWindows -or -not [Environment]::Is64BitProcess -or
        [Runtime.InteropServices.RuntimeInformation]::ProcessArchitecture -ne [Runtime.InteropServices.Architecture]::X64 -or
        $PSVersionTable.PSEdition -cne 'Core' -or $PSVersionTable.PSVersion.Major -ne 7 -or
        [IO.Path]::GetFileName([Environment]::ProcessPath) -ine 'pwsh.exe') { throw 'Provider runtime platform unsupported' }
    $handle=[IntPtr]::Zero
    $binding=$null
    try {
        $managed=Get-CovenProviderManagedCapture
        $path=Get-CovenProviderSystemLibraryPath -SystemDirectory ([Environment]::GetFolderPath([Environment+SpecialFolder]::System))
        $attributes=[IO.File]::GetAttributes($path)
        if (($attributes -band ([IO.FileAttributes]::ReparsePoint -bor [IO.FileAttributes]::Directory)) -ne 0) { throw 'library' }
        Assert-CovenProviderLoadedLibrary -Path $path -Required $false
        $hash=(Get-FileHash -LiteralPath $path -Algorithm SHA256 -ErrorAction Stop).Hash.ToLowerInvariant()
        $handle=[Runtime.InteropServices.NativeLibrary]::Load($path)
        if ($handle -eq [IntPtr]::Zero) { throw 'library' }
        $binding=[pscustomobject]@{Managed=$managed;LibraryPath=$path;LibrarySha256=$hash;Handle=$handle;Closed=$false;ReleaseAttempted=$false;ReleaseSucceeded=$false;ReleaseFailed=$false}
        if (-not (Test-CovenProviderRuntimeBinding -Binding $binding)) { throw 'binding' }
        return $binding
    } catch {
        if ($handle -ne [IntPtr]::Zero -and $null -ne $binding) {
            try { Close-CovenProviderRuntimeBinding -Binding $binding } catch { }
        }
        # Preserve the primary binding failure; a failed owned release never admits output.
        throw (New-CovenProviderBindingFailure -Binding $binding)
    }
}
function Test-CovenProviderRuntimeBinding {
    param([Parameter(Mandatory)]$Binding)
    try {
        if ($Binding.Closed -or $Binding.ReleaseAttempted -or $Binding.ReleaseFailed -or $Binding.ReleaseSucceeded -or $Binding.Handle -eq [IntPtr]::Zero -or -not (Test-CovenProviderManagedCapture -Capture $Binding.Managed)) { return $false }
        $attributes=[IO.File]::GetAttributes($Binding.LibraryPath)
        if (($attributes -band ([IO.FileAttributes]::ReparsePoint -bor [IO.FileAttributes]::Directory)) -ne 0) { return $false }
        Assert-CovenProviderLoadedLibrary -Path $Binding.LibraryPath -Required $true
        return (Get-FileHash -LiteralPath $Binding.LibraryPath -Algorithm SHA256 -ErrorAction Stop).Hash.ToLowerInvariant() -ceq $Binding.LibrarySha256
    } catch { return $false }
}
function Close-CovenProviderRuntimeBinding {
    param([Parameter(Mandatory)]$Binding)
    if ($Binding.Closed -or $Binding.Handle -eq [IntPtr]::Zero) { throw 'Provider runtime reference already closed' }
    if ($Binding.ReleaseAttempted) { throw 'Provider runtime reference release already attempted' }
    $Binding.ReleaseAttempted=$true
    try {
        Remove-CovenProviderNativeReference -Handle $Binding.Handle
        $Binding.Handle=[IntPtr]::Zero
        $Binding.ReleaseSucceeded=$true
        $Binding.Closed=$true
    } catch {
        $Binding.ReleaseFailed=$true
        throw 'Provider runtime reference release failed'
    }
}
function ConvertTo-CovenProviderSourceDto {
    param([Parameter(Mandatory)]$Capture)
    if (-not (Test-CovenProviderSourceCapture -Capture $Capture)) { throw 'Provider source binding changed' }
    $files=[Collections.Generic.List[CovenProviderBound.CovenKernelSourceFile]]::new()
    foreach ($file in @($Capture.Files | Sort-Object -CaseSensitive Id)) { $files.Add([CovenProviderBound.CovenKernelSourceFile]::new($file.Id,$file.Sha256)) }
    [CovenProviderBound.CovenKernelSource]::new($Capture.Commit,$files,$true)
}
function ConvertTo-CovenProviderHostDto {
    param([Parameter(Mandatory)]$Binding)
    if (-not (Test-CovenProviderRuntimeBinding -Binding $Binding)) { throw 'Provider host binding changed' }
    $assemblies=[Collections.Generic.List[CovenProviderBound.CovenKernelAssembly]]::new()
    foreach ($file in @($Binding.Managed.Files | Where-Object { $_.Kind -cne 'executable' } | Sort-Object -CaseSensitive Kind)) {
        $assemblies.Add([CovenProviderBound.CovenKernelAssembly]::new($file.Kind,$file.Sha256,[uint[]]$file.Version))
    }
    [CovenProviderBound.CovenKernelHost]::new($Binding.Managed.Files[0].Sha256,$assemblies,$Binding.LibrarySha256,$true)
}

function Get-CovenProviderSystemLibraryPath {
    param([AllowEmptyString()][string]$SystemDirectory)
    if ([string]::IsNullOrEmpty($SystemDirectory) -or -not [IO.Path]::IsPathFullyQualified($SystemDirectory)) { throw 'Provider system directory unavailable' }
    $path=Join-Path $SystemDirectory 'wevtapi.dll'
    if (-not [IO.Path]::IsPathFullyQualified($path)) { throw 'Provider system library path unavailable' }
    [IO.Path]::GetFullPath($path)
}
function Remove-CovenProviderNativeReference {
    param([IntPtr]$Handle)
    [Runtime.InteropServices.NativeLibrary]::Free($Handle)
}

function New-CovenProviderBindingFailure {
    param($Binding)
    $failure=[InvalidOperationException]::new('Provider runtime binding failed')
    $failure.Data['ProviderReleaseAttempted']=($null -ne $Binding -and [bool]$Binding.ReleaseAttempted)
    $failure.Data['ProviderReleaseFailed']=($null -ne $Binding -and [bool]$Binding.ReleaseFailed)
    return $failure
}
