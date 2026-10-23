# Private source buffers/paths never become public DTO properties.
function Get-CovenTokenSourceInventory {
    [ordered]@{
        'workflow'='.github/workflows/windows-reliability.yml'
        'runner'='scripts/windows-reliability/run-token-facts.ps1'
        'probe'='scripts/windows-reliability/token-facts-probe.ps1'
        'binding'='scripts/windows-reliability/token-binding.ps1'
        'binding-tests'='scripts/windows-reliability/token-binding-tests.ps1'
        'runtime-tests'='scripts/windows-reliability/token-runtime-tests.ps1'
        'binding-types'='scripts/windows-reliability/token-binding-types.cs'
        'binding-types-tests'='scripts/windows-reliability/token-binding-types-tests.ps1'
        'privilege-projector'='scripts/windows-reliability/token-privileges.cs'
        'privilege-controls'='scripts/windows-reliability/token-privileges-controls.cs'
        'privilege-tests'='scripts/windows-reliability/token-privileges-tests.ps1'
        'reader'='scripts/windows-reliability/token-reader.cs'
        'reader-controls'='scripts/windows-reliability/token-reader-controls.cs'
        'reader-tests'='scripts/windows-reliability/token-reader-tests.ps1'
        'native-adapter'='scripts/windows-reliability/token-native.cs'
        'native-controls'='scripts/windows-reliability/token-native-controls.cs'
        'native-tests'='scripts/windows-reliability/token-native-tests.ps1'
        'public-encoder'='scripts/windows-reliability/token-public.cs'
        'public-controls'='scripts/windows-reliability/token-public-controls.cs'
        'public-tests'='scripts/windows-reliability/token-public-tests.ps1'
        'query-helper'='scripts/windows-reliability/query-helper.ps1'
        'query-summary'='scripts/windows-reliability/query-summary.ps1'
        'source-stability'='scripts/windows-reliability/source-stability.ps1'
        'phase-validator'='scripts/windows-reliability/public_receipt.py'
        'result-validator'='scripts/windows-reliability/token_result.py'
        'result-tests'='scripts/windows-reliability/token_result_tests.py'
        'validator'='scripts/windows-reliability/token_receipt.py'
        'validator-tests'='scripts/windows-reliability/token_receipt_tests.py'
        'envelope-tests'='scripts/windows-reliability/token_envelope_tests.py'
        'workflow-controls'='scripts/windows-reliability/token-workflow-tests.py'
    }
}
function Read-CovenTokenSourceBytes {
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
            if ($count -gt $remaining) { throw 'Token source size limit' }
            $memory.Write($buffer,0,$count)
        }
        return ,$memory.ToArray()
    } finally {
        if ($null -ne $stream) { $stream.Dispose() }
        $memory.Dispose()
    }
}
function Get-CovenTokenSourceHash {
    param([Parameter(Mandatory)][AllowEmptyCollection()][byte[]]$Bytes)
    [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($Bytes)).ToLowerInvariant()
}
function Assert-CovenTokenSourceRoot {
    param([string]$Directory)
    if (-not [IO.Path]::IsPathFullyQualified($Directory)) { throw 'Token source root is not absolute' }
    $root=[IO.Path]::TrimEndingDirectorySeparator([IO.Path]::GetFullPath($Directory))
    # Inspect the chosen root itself. Platform aliases in its ancestors (for
    # example macOS /tmp) are outside this selected-root observation.
    $attributes=[IO.File]::GetAttributes($root)
    if (($attributes -band [IO.FileAttributes]::Directory) -eq 0 -or
        ($attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) { throw 'Token source root is not a plain directory' }
    $item=Get-Item -LiteralPath $root -Force -ErrorAction Stop
    if (-not [string]::IsNullOrEmpty($item.LinkType)) { throw 'Token source root is a link' }
    $root
}
function Assert-CovenTokenSourcePath {
    param([string]$Directory,[string]$Relative)
    $path=Assert-CovenTokenSourceRoot -Directory $Directory
    foreach ($part in $Relative.Split('/')) {
        $path=Join-Path $path $part
        $attributes=[IO.File]::GetAttributes($path)
        if (($attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) { throw 'Token source reparse point' }
        $item=Get-Item -LiteralPath $path -Force -ErrorAction Stop
        if (-not [string]::IsNullOrEmpty($item.LinkType)) { throw 'Token source link is not an owned plain path' }
    }
    if (($attributes -band [IO.FileAttributes]::Directory) -ne 0) { throw 'Token source is not a file' }
    $path
}
function Get-CovenTokenSourceCapture {
    param([Parameter(Mandatory)][string]$Directory,[Parameter(Mandatory)][string]$Commit)
    try {
        if ($Commit -cnotmatch '^[0-9a-f]{40}$') { throw 'commit' }
        $root=Assert-CovenTokenSourceRoot -Directory $Directory
        $inventory=Get-CovenTokenSourceInventory
        $files=[Collections.Generic.List[object]]::new()
        [long]$total=0
        foreach ($id in $inventory.Keys) {
            $path=Assert-CovenTokenSourcePath -Directory $root -Relative $inventory[$id]
            $bytes=Read-CovenTokenSourceBytes -Path $path
            $total+=$bytes.Length
            if ($total -gt 2097152) { throw 'total' }
            $files.Add([pscustomobject]@{Id=$id;Path=$path;Bytes=$bytes;Sha256=(Get-CovenTokenSourceHash -Bytes $bytes)})
        }
        [pscustomobject]@{Directory=$root;Commit=$Commit;Files=$files}
    } catch { throw 'Token source capture failed' }
}
function Test-CovenTokenSourceCapture {
    param([Parameter(Mandatory)]$Capture)
    try {
        $inventory=Get-CovenTokenSourceInventory
        if ($Capture.Commit -isnot [string] -or $Capture.Commit -cnotmatch '^[0-9a-f]{40}$' -or $Capture.Files.Count -ne $inventory.Count) { return $false }
        $expectedIds=[Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
        foreach ($id in $inventory.Keys) { $expectedIds.Add([string]$id) | Out-Null }
        $seen=[Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
        [long]$total=0
        foreach ($file in $Capture.Files) {
            if (-not $expectedIds.Contains($file.Id) -or -not $seen.Add($file.Id) -or $file.Bytes -isnot [byte[]]) { return $false }
            $path=Assert-CovenTokenSourcePath -Directory $Capture.Directory -Relative $inventory[$file.Id]
            if ($path -cne $file.Path -or $file.Bytes.Length -gt 262144) { return $false }
            $total+=$file.Bytes.Length
            if ($total -gt 2097152) { return $false }
            if ((Get-CovenTokenSourceHash -Bytes $file.Bytes) -cne $file.Sha256) { return $false }
            if ((Get-CovenTokenSourceHash -Bytes (Read-CovenTokenSourceBytes -Path $path)) -cne $file.Sha256) { return $false }
        }
        return $seen.SetEquals($expectedIds)
    } catch { return $false }
}
function Add-CovenTokenCapturedTypes {
    param([Parameter(Mandatory)]$Capture)
    if (-not (Test-CovenTokenSourceCapture -Capture $Capture)) { throw 'Token source changed before compilation' }
    if ('CovenTokenBound.CovenOwnedTokenCollector' -as [type]) { throw 'Token compiled type already present' }
    $utf8=[Text.UTF8Encoding]::new($false,$true)
    $parts=foreach ($id in @('privilege-projector','reader','native-adapter','public-encoder','binding-types')) {
        $file=@($Capture.Files | Where-Object { $_.Id -ceq $id })
        if ($file.Count -ne 1) { throw 'Token compilation source missing' }
        # Keep each captured compilation unit's usings inside the same fixed namespace.
        "namespace CovenTokenBound {`n"+$utf8.GetString($file[0].Bytes)+"`n}"
    }
    $references=@(Get-ChildItem -LiteralPath (Join-Path $PSHOME 'ref') -Filter '*.dll' | ForEach-Object FullName)
    Add-Type -TypeDefinition ($parts -join "`n") -ReferencedAssemblies $references -ErrorAction Stop
    if (-not (Test-CovenTokenSourceCapture -Capture $Capture)) { throw 'Token source changed during compilation' }
}

# Selected runtime identities only, not compiler/all-assembly attestation.
function Get-CovenTokenManagedCapture {
    $types=[ordered]@{corelib=[object];powershell=[System.Management.Automation.PSObject];json=[Text.Json.JsonSerializer];crypto=[Security.Cryptography.SHA256]}
    $files=[Collections.Generic.List[object]]::new()
    $executable=[Environment]::ProcessPath
    if(-not [IO.Path]::IsPathFullyQualified($executable)){throw 'Token executable unavailable'}
    $files.Add([pscustomobject]@{Kind='executable';Path=$executable;Sha256=(Get-FileHash -LiteralPath $executable -Algorithm SHA256 -ErrorAction Stop).Hash.ToLowerInvariant();Version=@()})
    foreach($kind in $types.Keys){
        $assembly=$types[$kind].Assembly;$path=$assembly.Location;$version=$assembly.GetName().Version
        if(-not [IO.Path]::IsPathFullyQualified($path)){throw 'Token managed identity unavailable'}
        $parts=@($version.Major,$version.Minor,$version.Build,$version.Revision)
        if(@($parts | Where-Object {$_ -lt 0}).Count -ne 0){throw 'Token managed version unavailable'}
        $files.Add([pscustomobject]@{Kind=$kind;Path=$path;Sha256=(Get-FileHash -LiteralPath $path -Algorithm SHA256 -ErrorAction Stop).Hash.ToLowerInvariant();Version=$parts})
    }
    [pscustomobject]@{Files=$files}
}
function Test-CovenTokenManagedCapture {
    param([Parameter(Mandatory)]$Capture)
    try {
        $current=Get-CovenTokenManagedCapture
        if($Capture.Files.Count -ne $current.Files.Count){return $false}
        for($i=0;$i -lt $current.Files.Count;$i++){
            $before=$Capture.Files[$i];$after=$current.Files[$i]
            if($before.Kind -cne $after.Kind -or $before.Path -cne $after.Path -or $before.Sha256 -cne $after.Sha256 -or ($before.Version -join ',') -cne ($after.Version -join ',')){return $false}
        }
        return $true
    } catch {return $false}
}
function Get-CovenTokenSystemLibraryPath {
    param([string]$SystemDirectory,[ValidateSet('advapi32.dll','kernel32.dll')][string]$LibraryName)
    if([string]::IsNullOrEmpty($SystemDirectory) -or -not [IO.Path]::IsPathFullyQualified($SystemDirectory)){throw 'Token system directory unavailable'}
    [IO.Path]::GetFullPath((Join-Path $SystemDirectory $LibraryName))
}
function Get-CovenTokenLoadedModuleSnapshot {
    $process=[Diagnostics.Process]::GetCurrentProcess()
    try {
        $modules=$process.Modules
        if($modules.Count -gt 4096){throw 'Token module observation limit'}
        $snapshot=[Collections.Generic.List[object]]::new()
        foreach($module in $modules){
            if($module.ModuleName -iin @('advapi32.dll','kernel32.dll')){$snapshot.Add([pscustomobject]@{ModuleName=$module.ModuleName;FileName=$module.FileName})}
        }
        return ,$snapshot.ToArray()
    } finally {$process.Dispose()}
}
function Test-CovenTokenLoadedLibrary {
    param([string]$ExpectedPath,[ValidateSet('advapi32.dll','kernel32.dll')][string]$LibraryName,[AllowEmptyCollection()][object[]]$Modules,[bool]$Required)
    try {
        if($Modules.Count -gt 4096){return $false}
        $matches=@($Modules | Where-Object {[string]::Equals($_.ModuleName,$LibraryName,[StringComparison]::OrdinalIgnoreCase)})
        if($matches.Count -eq 0){return (-not $Required)}
        return $matches.Count -eq 1 -and [string]::Equals([IO.Path]::GetFullPath($matches[0].FileName),$ExpectedPath,[StringComparison]::OrdinalIgnoreCase)
    } catch {return $false}
}
function Assert-CovenTokenLoadedLibrary {
    param([string]$Path,[string]$LibraryName,[bool]$Required)
    $modules=Get-CovenTokenLoadedModuleSnapshot
    if(-not (Test-CovenTokenLoadedLibrary -ExpectedPath $Path -LibraryName $LibraryName -Modules $modules -Required $Required)){throw 'Token native module identity mismatch'}
}
function Add-CovenTokenNativeReference {
    param([string]$Path)
    [Runtime.InteropServices.NativeLibrary]::Load($Path)
}
function Remove-CovenTokenNativeReference {
    param([IntPtr]$Handle)
    [Runtime.InteropServices.NativeLibrary]::Free($Handle)
}
function New-CovenTokenBindingFailure {
    param($Binding)
    $failure=[InvalidOperationException]::new('Token runtime binding failed')
    $failure.Data['TokenReleaseAttempted']=($null -ne $Binding -and [bool]$Binding.ReleaseAttempted)
    $failure.Data['TokenReleaseFailed']=($null -ne $Binding -and [bool]$Binding.ReleaseFailed)
    $failure.Data['TokenBindingStage']=if($null -ne $Binding){$Binding.Stage}else{'bootstrap'}
    return $failure
}
function Open-CovenTokenRuntimeReferences {
    param([Parameter(Mandatory)]$Managed,[Parameter(Mandatory)][string]$SystemDirectory)
    $binding=[pscustomobject]@{Managed=$Managed;SystemDirectory=$SystemDirectory;Libraries=[Collections.Generic.List[object]]::new();Stage='discovery';Closed=$false;ReleaseAttempted=$false;ReleaseSucceeded=$false;ReleaseFailed=$false}
    try {
        # Discover both libraries before acquiring any new reference.
        foreach($name in @('advapi32.dll','kernel32.dll')){
            $path=Get-CovenTokenSystemLibraryPath -SystemDirectory $SystemDirectory -LibraryName $name
            $attributes=[IO.File]::GetAttributes($path)
            if(($attributes -band ([IO.FileAttributes]::ReparsePoint -bor [IO.FileAttributes]::Directory)) -ne 0){throw 'library'}
            Assert-CovenTokenLoadedLibrary -Path $path -LibraryName $name -Required $false
            $kind=[IO.Path]::GetFileNameWithoutExtension($name)+'-system32'
            $binding.Libraries.Add([pscustomobject]@{Name=$name;Kind=$kind;Path=$path;Sha256=(Get-FileHash -LiteralPath $path -Algorithm SHA256 -ErrorAction Stop).Hash.ToLowerInvariant();Handle=[IntPtr]::Zero;ReleaseAttempted=$false;ReleaseSucceeded=$false;ReleaseFailed=$false})
        }
        $binding.Stage='reference'
        foreach($library in $binding.Libraries){
            $loaded=Add-CovenTokenNativeReference -Path $library.Path
            if($loaded -isnot [IntPtr] -or $loaded -eq [IntPtr]::Zero -or $loaded -eq [IntPtr]::new(-1) -or $loaded -eq [IntPtr]::new(-2)){throw 'library'}
            # The pre-registered entry owns this successfully returned reference.
            $library.Handle=$loaded
        }
        $binding.Stage='verification'
        if(-not (Test-CovenTokenRuntimeBinding $binding)){throw 'binding'}
        return $binding
    } catch {
        if(@($binding.Libraries | Where-Object {$_.Handle -ne [IntPtr]::Zero}).Count -gt 0 -and -not $binding.ReleaseAttempted){try{Close-CovenTokenRuntimeBinding $binding}catch{}}
        throw (New-CovenTokenBindingFailure $binding)
    }
}
function Open-CovenTokenRuntimeBinding {
    if(-not $IsWindows -or -not [Environment]::Is64BitProcess -or [Runtime.InteropServices.RuntimeInformation]::ProcessArchitecture -ne [Runtime.InteropServices.Architecture]::X64 -or $PSVersionTable.PSEdition -cne 'Core' -or $PSVersionTable.PSVersion.Major -ne 7 -or [IO.Path]::GetFileName([Environment]::ProcessPath) -ine 'pwsh.exe'){throw 'Token runtime platform unsupported'}
    $managed=Get-CovenTokenManagedCapture
    Open-CovenTokenRuntimeReferences -Managed $managed -SystemDirectory ([Environment]::GetFolderPath([Environment+SpecialFolder]::System))
}
function Test-CovenTokenRuntimeBinding {
    param([Parameter(Mandatory)]$Binding)
    try {
        if($Binding.Closed -or $Binding.ReleaseAttempted -or $Binding.ReleaseFailed -or $Binding.ReleaseSucceeded -or $Binding.Libraries.Count -ne 2 -or -not (Test-CovenTokenManagedCapture $Binding.Managed)){return $false}
        $expected=@('advapi32.dll','kernel32.dll')
        for($i=0;$i -lt 2;$i++){
            $library=$Binding.Libraries[$i]
            if($library.Name -cne $expected[$i] -or $library.Kind -cne ([IO.Path]::GetFileNameWithoutExtension($expected[$i])+'-system32') -or $library.Path -cne (Get-CovenTokenSystemLibraryPath -SystemDirectory $Binding.SystemDirectory -LibraryName $expected[$i]) -or $library.Handle -eq [IntPtr]::Zero -or $library.ReleaseAttempted -or $library.ReleaseFailed -or $library.ReleaseSucceeded){return $false}
            $attributes=[IO.File]::GetAttributes($library.Path)
            if(($attributes -band ([IO.FileAttributes]::ReparsePoint -bor [IO.FileAttributes]::Directory)) -ne 0){return $false}
            Assert-CovenTokenLoadedLibrary -Path $library.Path -LibraryName $library.Name -Required $true
            if((Get-FileHash -LiteralPath $library.Path -Algorithm SHA256 -ErrorAction Stop).Hash.ToLowerInvariant() -cne $library.Sha256){return $false}
        }
        return $true
    } catch {return $false}
}
function Close-CovenTokenRuntimeBinding {
    param([Parameter(Mandatory)]$Binding)
    if($Binding.Closed -or $Binding.ReleaseAttempted){throw 'Token runtime reference release already attempted'}
    $Binding.ReleaseAttempted=$true;$failed=$false
    for($i=$Binding.Libraries.Count-1;$i -ge 0;$i--){
        $library=$Binding.Libraries[$i]
        if($library.Handle -eq [IntPtr]::Zero){continue}
        $library.ReleaseAttempted=$true
        try {
            Remove-CovenTokenNativeReference -Handle $library.Handle
            $library.Handle=[IntPtr]::Zero;$library.ReleaseSucceeded=$true
        } catch {$library.ReleaseFailed=$true;$failed=$true}
    }
    $Binding.ReleaseFailed=$failed;$Binding.ReleaseSucceeded=(-not $failed);$Binding.Closed=(-not $failed)
    if($failed){throw 'Token runtime reference release failed'}
}
function ConvertTo-CovenTokenSourceDto {
    param([Parameter(Mandatory)]$Capture)
    if(-not (Test-CovenTokenSourceCapture $Capture)){throw 'Token source binding changed'}
    $ids=[string[]]@((Get-CovenTokenSourceInventory).Keys);[Array]::Sort($ids,[Collections.IComparer][StringComparer]::Ordinal)
    $files=[Collections.Generic.List[CovenTokenBound.CovenTokenSourceFileIdentity]]::new()
    foreach($id in $ids){$file=@($Capture.Files | Where-Object {$_.Id -ceq $id})[0];$files.Add([CovenTokenBound.CovenTokenSourceFileIdentity]::new($file.Id,$file.Sha256))}
    [CovenTokenBound.CovenTokenSourceIdentity]::new($Capture.Commit,$files.ToArray(),$true)
}
function ConvertTo-CovenTokenHostDto {
    param([Parameter(Mandatory)]$Binding)
    if(-not (Test-CovenTokenRuntimeBinding $Binding)){throw 'Token runtime binding changed'}
    $assemblies=[Collections.Generic.List[CovenTokenBound.CovenTokenAssemblyIdentity]]::new()
    $kinds=[string[]]@('corelib','powershell','json','crypto');[Array]::Sort($kinds,[Collections.IComparer][StringComparer]::Ordinal)
    foreach($kind in $kinds){$file=@($Binding.Managed.Files | Where-Object {$_.Kind -ceq $kind})[0];$assemblies.Add([CovenTokenBound.CovenTokenAssemblyIdentity]::new($kind,$file.Sha256,[uint[]]$file.Version))}
    $libraries=[Collections.Generic.List[CovenTokenBound.CovenTokenLibraryIdentity]]::new()
    foreach($library in $Binding.Libraries){$libraries.Add([CovenTokenBound.CovenTokenLibraryIdentity]::new($library.Kind,$library.Sha256))}
    [CovenTokenBound.CovenTokenHostIdentity]::new($Binding.Managed.Files[0].Sha256,$assemblies.ToArray(),$libraries.ToArray(),$true)
}
function ConvertTo-CovenTokenPublicIdentity {
    param([Parameter(Mandatory)]$Value)
    $bytes=[CovenTokenBound.CovenTokenBindingPublic]::EncodeIdentity($Value)
    $text=[Text.UTF8Encoding]::new($false,$true).GetString($bytes)
    ConvertFrom-Json -InputObject $text -AsHashtable -Depth 8 -ErrorAction Stop
}

function ConvertTo-CovenTokenQuerySummary {
    param([Parameter(Mandatory)]$Result)
    if($Result.GetType() -ne [CovenQueryResult]){throw 'Token query result type rejected'}
    $observation=ConvertTo-CovenQueryObservation -Result $Result
    $summary=[ordered]@{kind='effective-token-facts';stdoutDigestScope='decoded-text-utf8'}
    foreach($property in $observation.PSObject.Properties){$summary[$property.Name]=$property.Value}
    [pscustomobject]$summary
}
function New-CovenTokenPrivatePacket {
    param([Parameter(Mandatory)]$Source,[Parameter(Mandatory)]$HostProof,
        [Parameter(Mandatory)]$Query)
    try {
        # Inspect before PowerShell parameter coercion can mint a result object.
        if($Query.GetType() -ne [CovenQueryResult]){throw 'type'}
        # The query helper owns bounded raw strings. Never serialize the native
        # result itself; copy only its closed numeric/digest observation surface.
        $probe=[string]$Query.Stdout
        if($probe.Length -eq 0 -or $probe.Length -gt 32768 -or $probe -cmatch '[^\x00-\x7f]' -or ([string]$Query.Stderr).Length -gt 32768){throw 'size'}
        $packet=[ordered]@{source=(ConvertTo-CovenTokenPublicIdentity $Source);host=(ConvertTo-CovenTokenPublicIdentity $HostProof);
            query=(ConvertTo-CovenTokenQuerySummary $Query);probe=$probe}
        $bytes=[Text.UTF8Encoding]::new($false,$true).GetBytes(($packet | ConvertTo-Json -Depth 16 -Compress))
        if($bytes.Length -gt 131072){throw 'size'}
        return ,$bytes
    } catch {throw 'Token private packet failed'}
}

function New-CovenTokenProbeInvocation {
    param([Parameter(Mandatory)]$Capture)
    try {
        if(-not (Test-CovenTokenSourceCapture $Capture)){throw 'source'}
        $probe=@($Capture.Files | Where-Object {$_.Id -ceq 'probe'})
        if($probe.Count -ne 1){throw 'source'}
        # Validate the exact bytes before encoding; -File would reread the path.
        $utf8=[Text.UTF8Encoding]::new($false,$true)
        $null=$utf8.GetString($probe[0].Bytes)
        $probe64=[Convert]::ToBase64String($probe[0].Bytes)
        $hash=Get-CovenTokenSourceHash $probe[0].Bytes
        $root64=[Convert]::ToBase64String($utf8.GetBytes($Capture.Directory))
        $commit=$Capture.Commit
        $bootstrap=@"
`$ErrorActionPreference='Stop'
`$bytes=[Convert]::FromBase64String('$probe64')
`$directory=[Text.UTF8Encoding]::new(`$false,`$true).GetString([Convert]::FromBase64String('$root64'))
`$probe=[scriptblock]::Create([Text.UTF8Encoding]::new(`$false,`$true).GetString(`$bytes))
& `$probe -Directory `$directory -Commit '$commit' -ProbeSourceHash '$hash'
"@
        $encoded=[Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($bootstrap))
        # Leaves headroom for a separately bounded executable and fixed flags
        # below Windows' command-line limit. This is not a response deadline.
        if($encoded.Length -gt 24576){throw 'command size'}
        [pscustomobject]@{Arguments=[string[]]@('-NoLogo','-NoProfile','-NonInteractive','-EncodedCommand',$encoded);ProbeSha256=$hash}
    } catch {throw 'Token probe invocation failed'}
}

function New-CovenTokenPublisherInvocation {
    param([Parameter(Mandatory)]$Capture,[Parameter(Mandatory)][string]$Executable,
        [Parameter(Mandatory)][string]$BundlePath,[Parameter(Mandatory)][string]$TemporaryRoot,
        [Parameter(Mandatory)][string]$OutputDirectory)
    try {
        if(-not (Test-CovenTokenSourceCapture $Capture)){throw 'source'}
        foreach($path in @($Executable,$BundlePath,$TemporaryRoot,$OutputDirectory,$Capture.Directory)){
            if(-not [IO.Path]::IsPathFullyQualified($path) -or $path.Length -gt 2048 -or $path.Contains([char]0)){throw 'path'}
        }
        $file=@($Capture.Files | Where-Object {$_.Id -ceq 'validator'})
        if($file.Count -ne 1 -or $file[0].Bytes.Length -eq 0 -or $file[0].Bytes.Length -gt 262144){throw 'source'}
        $null=[Text.UTF8Encoding]::new($false,$true).GetString($file[0].Bytes)
        $hash=Get-CovenTokenSourceHash $file[0].Bytes
        $memory=[IO.MemoryStream]::new()
        try {
            $gzip=[IO.Compression.GZipStream]::new($memory,[IO.Compression.CompressionLevel]::Optimal,$true)
            try {$gzip.Write($file[0].Bytes,0,$file[0].Bytes.Length)} finally {$gzip.Dispose()}
            $encoded=[Convert]::ToBase64String($memory.ToArray())
        } finally {$memory.Dispose()}
        $bootstrap=@"
import base64,hashlib,sys,zlib
try:
 b=base64.b64decode('$encoded',validate=True)
 d=zlib.decompressobj(31);s=d.decompress(b,262145)
 if not (0<len(s)<=262144 and d.eof and not d.unused_data and not d.unconsumed_tail and hashlib.sha256(s).hexdigest()=='$hash'):raise ValueError()
 scope={'__name__':'coven_captured_token_publisher'}
 exec(compile(s,'<coven-token-publisher>','exec',dont_inherit=True),scope)
 code=scope['run_token_publisher'](sys.argv[1:],'$hash')
except Exception:
 print('token-publication-failed cleanupFailed=false',file=sys.stderr)
 code=1
raise SystemExit(code)
"@
        $arguments=[string[]]@('-B','-c',$bootstrap,$BundlePath,$TemporaryRoot,$OutputDirectory,$Capture.Directory)
        # Twice every character plus surrounding quotes/separators bounds the
        # Windows escaped command, including backslashes before quotes/end.
        $budget=2*$Executable.Length+3
        foreach($argument in $arguments){$budget+=2*$argument.Length+3}
        if($budget -gt 32760){throw 'command size'}
        [pscustomobject]@{Arguments=$arguments;PublisherSha256=$hash;CommandCharacterBound=$budget}
    } catch {throw 'Token publisher invocation failed'}
}

function New-CovenTokenRunnerInvocation {
    param([Parameter(Mandatory)]$Capture,[Parameter(Mandatory)][string]$OutputDirectory,
        [Parameter(Mandatory)][string]$TemporaryRoot,[Parameter(Mandatory)][string]$Executable)
    try {
        if(-not (Test-CovenTokenSourceCapture $Capture)){throw 'source'}
        foreach($path in @($Capture.Directory,$OutputDirectory,$TemporaryRoot,$Executable)){
            if(-not [IO.Path]::IsPathFullyQualified($path) -or $path.Length -gt 2048 -or $path.Contains([char]0)){throw 'path'}
        }
        $runner=@($Capture.Files|Where-Object {$_.Id -ceq 'runner'})
        if($runner.Count -ne 1 -or $runner[0].Bytes.Length -eq 0 -or $runner[0].Bytes.Length -gt 262144){throw 'source'}
        $utf8=[Text.UTF8Encoding]::new($false,$true);$null=$utf8.GetString($runner[0].Bytes)
        $hash=Get-CovenTokenSourceHash $runner[0].Bytes
        $memory=[IO.MemoryStream]::new()
        try {
            $gzip=[IO.Compression.GZipStream]::new($memory,[IO.Compression.CompressionLevel]::Optimal,$true)
            try {$gzip.Write($runner[0].Bytes,0,$runner[0].Bytes.Length)} finally {$gzip.Dispose()}
            $encoded=[Convert]::ToBase64String($memory.ToArray())
        } finally {$memory.Dispose()}
        $parameters=[ordered]@{Directory=$Capture.Directory;Commit=$Capture.Commit;OutputDirectory=$OutputDirectory;TemporaryRoot=$TemporaryRoot;RunnerSourceHash=$hash}
        $parameters64=[Convert]::ToBase64String($utf8.GetBytes(($parameters|ConvertTo-Json -Compress)))
        $bootstrap=@"
`$ErrorActionPreference='Stop'
try {
 `$inputStream=[IO.MemoryStream]::new([Convert]::FromBase64String('$encoded'))
 `$outputStream=[IO.MemoryStream]::new();`$gzip=`$null
 try {
  `$gzip=[IO.Compression.GZipStream]::new(`$inputStream,[IO.Compression.CompressionMode]::Decompress)
  `$buffer=[byte[]]::new(8192)
  while(`$true){
   `$remaining=262144-`$outputStream.Length
   `$count=`$gzip.Read(`$buffer,0,[int][Math]::Min(`$buffer.Length,`$remaining+1))
   if(`$count -eq 0){break}
   if(`$count -gt `$remaining){throw 'size'}
   `$outputStream.Write(`$buffer,0,`$count)
  }
  `$bytes=`$outputStream.ToArray()
 } finally {if(`$null -ne `$gzip){`$gzip.Dispose()};`$inputStream.Dispose();`$outputStream.Dispose()}
 if(`$bytes.Length -eq 0 -or [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData(`$bytes)).ToLowerInvariant() -cne '$hash'){throw 'source'}
 `$utf8=[Text.UTF8Encoding]::new(`$false,`$true)
 `$parameters=ConvertFrom-Json -InputObject (`$utf8.GetString([Convert]::FromBase64String('$parameters64'))) -AsHashtable -Depth 4
 & ([scriptblock]::Create(`$utf8.GetString(`$bytes))) @parameters
} catch {[Console]::Error.Write("token-runner-bootstrap-failed``n");exit 1}
"@
        $command=[Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($bootstrap))
        if($command.Length -gt 24576){throw 'command size'}
        [pscustomobject]@{Arguments=[string[]]@('-NoLogo','-NoProfile','-NonInteractive','-EncodedCommand',$command);RunnerSha256=$hash}
    } catch {throw 'Token runner invocation failed'}
}

function New-CovenTokenArtifactInvocation {
    param([Parameter(Mandatory)]$Capture,[Parameter(Mandatory)][string]$Executable,
        [Parameter(Mandatory)][string]$SourcePath,[Parameter(Mandatory)][string]$SnapshotHash,
        [Parameter(Mandatory)][string]$TemporaryRoot,[Parameter(Mandatory)][string]$OutputDirectory)
    try {
        if($SnapshotHash -cnotmatch '^[0-9a-f]{64}$'){throw 'snapshot'}
        $invocation=New-CovenTokenPublisherInvocation -Capture $Capture -Executable $Executable -BundlePath $SourcePath -TemporaryRoot $TemporaryRoot -OutputDirectory $OutputDirectory
        $from="scope['run_token_publisher']";$to="scope['run_token_artifact_admission']"
        if($invocation.Arguments[2].Split($from,[StringSplitOptions]::None).Count -ne 2){throw 'entry'}
        $arguments=[string[]]@($invocation.Arguments)
        $arguments[2]=$arguments[2].Replace($from,$to)
        $arguments=[string[]]@($arguments+$SnapshotHash)
        $budget=2*$Executable.Length+3
        foreach($argument in $arguments){$budget+=2*$argument.Length+3}
        if($budget -gt 32760){throw 'command size'}
        [pscustomobject]@{Arguments=$arguments;PublisherSha256=$invocation.PublisherSha256;CommandCharacterBound=$budget}
    } catch {throw 'Token artifact invocation failed'}
}
