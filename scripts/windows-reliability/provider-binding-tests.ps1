param([string]$BundleOutputFile)
$ErrorActionPreference='Stop'
$binding=Join-Path $PSScriptRoot 'provider-binding.ps1'
if (-not (Test-Path -LiteralPath $binding)) { throw 'Provider source binding helper missing' }
. $binding
$inventory=Get-CovenProviderSourceInventory
$expected=@('workflow','runner','probe','binding','binding-tests','reader','template','public-encoder','metadata-tests','controlled-api','template-tests','adapter-tests','encoder-tests','controlled-encoder','query-helper','query-summary','source-stability','phase-validator','validator','validator-tests','envelope-tests') | Sort-Object
$actualIds=($inventory.Keys | Sort-Object) -join ','
$expectedIds=$expected -join ','
if ($actualIds -cne $expectedIds) { throw 'Closed source inventory drift' }
$root=Join-Path ([IO.Path]::GetTempPath()) ('coven-provider-binding-control-'+[Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $root | Out-Null
try {
    foreach ($id in $inventory.Keys) {
        $file=Join-Path $root $inventory[$id]
        New-Item -ItemType Directory -Path ([IO.Path]::GetDirectoryName($file)) -Force | Out-Null
        if ($id -in @('template','reader','public-encoder')) {
            $name=[IO.Path]::GetFileName($inventory[$id])
            [IO.File]::WriteAllBytes($file,[IO.File]::ReadAllBytes((Join-Path $PSScriptRoot $name)))
        } else { [IO.File]::WriteAllText($file,'controlled source') }
    }
    # The initial complete positive also preserves relative-root normalization.
    $savedCurrent=[Environment]::CurrentDirectory
    try {
        [Environment]::CurrentDirectory=$root
        $capture=Get-CovenProviderSourceCapture -Directory '.' -Commit ('1'*40)
    } finally { [Environment]::CurrentDirectory=$savedCurrent }
    if ($capture.Files.Count -ne 21) { throw 'Capture omitted source' }
    if ($BundleOutputFile) {
        # This integration fixture exercises the packet and refusal boundary.
        # The separately required default invocation runs every component control.
        Add-CovenProviderCapturedTypes -Capture $capture
        # Exercise the actual parent packet boundary with controlled identities only.
        $source=ConvertTo-CovenProviderSourceDto -Capture $capture
        $assemblies=[Collections.Generic.List[CovenProviderBound.CovenKernelAssembly]]::new()
        foreach ($kind in @('corelib','crypto','json','powershell','xml')) {
            $major=if ($kind -ceq 'powershell') { 7 } else { 8 }
            $assemblies.Add([CovenProviderBound.CovenKernelAssembly]::new($kind,('3'*64),[uint[]]@($major,0,0,0)))
        }
        $hostProof=[CovenProviderBound.CovenKernelHost]::new(('2'*64),$assemblies,('4'*64),$true)
        $summary=[ordered]@{kind='controlled-parent-packet'}
        $query=[pscustomobject]@{Stdout='{}'}
        $utf8=[Text.UTF8Encoding]::new($false,$true)
        $runner=[IO.File]::ReadAllText((Join-Path $PSScriptRoot 'run-provider-schema.ps1'))
        $start=$runner.IndexOf('    $packet=[ordered]@',[StringComparison]::Ordinal)
        $end=$runner.IndexOf('    if ($packetBytes.Length',$start,[StringComparison]::Ordinal)
        if ($start -lt 0 -or $end -le $start) { throw 'Parent packet boundary missing' }
        . ([scriptblock]::Create($runner.Substring($start,$end-$start)))
        [IO.File]::WriteAllBytes($BundleOutputFile,$packetBytes)
        foreach ($candidate in @($capture,[pscustomobject]@{private='PRIVATE_SENTINEL'},
            [CovenProviderBound.CovenKernelSource]::new(('x'*8192),$source.Files,$true))) {
            $rejected=$false
            try { ConvertTo-CovenProviderPublicObject -Value $candidate | Out-Null } catch { $rejected=$true }
            if (-not $rejected) { throw 'Private or oversized identity admitted' }
        }

        return
    }
    # All root negatives use this already-valid complete inventory. A missing
    # descendant must never make a root-admission control pass accidentally.
    function New-ControlledProviderRootAlias {
        param([string]$Path,[string]$Target)
        $kind=if ($IsWindows) { 'Junction' } else { 'SymbolicLink' }
        New-Item -ItemType $kind -Path $Path -Target $Target -ErrorAction Stop | Out-Null
        if ((Get-Item -LiteralPath $Path -Force).LinkType -cne $kind) { throw 'Owned root alias setup failed' }
    }
    $rootFailures=[Collections.Generic.List[string]]::new()
    $rootAlias=$root+'-alias'
    $savedProviderRootRead=${function:Read-CovenProviderSourceBytes}
    $script:providerRootEntryReads=0
    try {
        function Read-CovenProviderSourceBytes {
            param([string]$Path)
            $script:providerRootEntryReads++
            return ,([byte[]](& $savedProviderRootRead -Path $Path))
        }
        New-ControlledProviderRootAlias -Path $rootAlias -Target $root
        $rejected=$false
        try { Get-CovenProviderSourceCapture -Directory $rootAlias -Commit ('1'*40) | Out-Null } catch { $rejected=$true }
        if (-not $rejected) { $rootFailures.Add('selected-root-alias') }
        if ($script:providerRootEntryReads -ne 0) { $rootFailures.Add('selected-root-read-before-admission') }
    } finally {
        Set-Item -Path function:Read-CovenProviderSourceBytes -Value $savedProviderRootRead
        if ([IO.Directory]::Exists($rootAlias)) { [IO.Directory]::Delete($rootAlias) }
    }
    $backup=$root+'-backup'
    $script:providerRootEntryReads=0
    try {
        function Read-CovenProviderSourceBytes {
            param([string]$Path)
            $script:providerRootEntryReads++
            return ,([byte[]](& $savedProviderRootRead -Path $Path))
        }
        [IO.Directory]::Move($root,$backup)
        New-ControlledProviderRootAlias -Path $root -Target $backup
        if (Test-CovenProviderSourceCapture -Capture $capture) { $rootFailures.Add('recheck-root-replacement') }
        if ($script:providerRootEntryReads -ne 0) { $rootFailures.Add('recheck-root-read-before-admission') }
    } finally {
        Set-Item -Path function:Read-CovenProviderSourceBytes -Value $savedProviderRootRead
        if ([IO.Directory]::Exists($backup)) {
            if ([IO.Directory]::Exists($root)) { [IO.Directory]::Delete($root) }
            [IO.Directory]::Move($backup,$root)
        }
    }
    $savedProviderRead=${function:Read-CovenProviderSourceBytes}
    foreach ($operation in @('capture','recheck')) {
        $script:providerRootReadCount=0
        try {
            function Read-CovenProviderSourceBytes {
                param([string]$Path)
                [byte[]]$data=& $savedProviderRead -Path $Path
                $script:providerRootReadCount++
                if ($script:providerRootReadCount -eq $inventory.Count) {
                    [IO.Directory]::Move($root,$backup)
                    New-ControlledProviderRootAlias -Path $root -Target $backup
                }
                return ,$data
            }
            $rejected=$false
            if ($operation -ceq 'capture') {
                try { Get-CovenProviderSourceCapture -Directory $root -Commit ('1'*40) | Out-Null } catch { $rejected=$true }
            } else { $rejected=-not (Test-CovenProviderSourceCapture -Capture $capture) }
            if ($script:providerRootReadCount -ne $inventory.Count) { throw 'Final-read replacement control did not execute' }
            if (-not $rejected) { $rootFailures.Add($operation+'-final-root-replacement') }
        } finally {
            Set-Item -Path function:Read-CovenProviderSourceBytes -Value $savedProviderRead
            if ([IO.Directory]::Exists($backup)) {
                if ([IO.Directory]::Exists($root)) { [IO.Directory]::Delete($root) }
                [IO.Directory]::Move($backup,$root)
            }
        }
    }
    if (-not (Test-CovenProviderSourceCapture -Capture $capture)) { throw 'Restored plain source root rejected' }
    if ($rootFailures.Count -gt 0) { throw ('Provider root controls admitted: '+($rootFailures -join ',')) }
    $runnerFile=@($capture.Files | Where-Object { $_.Id -ceq 'runner' })[0]
    $admittedCases=[Collections.Generic.List[string]]::new()
    foreach ($mode in @('case-only','case-alias-with-omission')) {
        $copyFiles=@(foreach ($file in $capture.Files) {
            if ($mode -ceq 'case-only' -and $file.Id -ceq 'runner') {
                [pscustomobject]@{Id='RUNNER';Path=$file.Path;Bytes=$file.Bytes;Sha256=$file.Sha256}
            } elseif ($mode -ceq 'case-alias-with-omission' -and $file.Id -ceq 'binding') {
                [pscustomobject]@{Id='RUNNER';Path=$runnerFile.Path;Bytes=$runnerFile.Bytes;Sha256=$runnerFile.Sha256}
            } else { $file }
        })
        $alias=[pscustomobject]@{Directory=$capture.Directory;Commit=$capture.Commit;Files=$copyFiles}
        if (Test-CovenProviderSourceCapture -Capture $alias) { $admittedCases.Add($mode) }
    }

    if ($admittedCases.Count -gt 0) { throw ('Case aliases admitted: '+($admittedCases -join ',')) }
    $readerPath=Join-Path $root $inventory['reader']
    $bytes=[IO.File]::ReadAllBytes($readerPath)
    [IO.File]::AppendAllText($readerPath,"`n// changed source")
    $rejected=$false
    try { Add-CovenProviderCapturedTypes -Capture $capture } catch { $rejected=$true }
    if (-not $rejected) { throw 'Changed source accepted before compilation' }
    [IO.File]::WriteAllBytes($readerPath,$bytes)
    $capture=Get-CovenProviderSourceCapture -Directory $root -Commit ('1'*40)
    Add-CovenProviderCapturedTypes -Capture $capture
    $result=[CovenProviderBound.CovenKernelTemplate]::Project('<template><data name="Irp" inType="win:Pointer"/></template>',2048)
    if ($result.Status -cne 'supported-flat') { throw 'Captured source compilation changed projector semantics' }
    if (-not (Test-CovenProviderSourceCapture -Capture $capture)) { throw 'Unchanged captured source rejected' }

    [IO.File]::AppendAllText($readerPath,"`n// changed after compilation")
    if (Test-CovenProviderSourceCapture -Capture $capture) { throw 'Post-compilation mutation accepted' }
    [IO.File]::WriteAllBytes($readerPath,$bytes)
    $capture=Get-CovenProviderSourceCapture -Directory $root -Commit ('1'*40)
    $alias=Join-Path $root 'reader-alias'
    New-Item -ItemType HardLink -Path $alias -Target $readerPath | Out-Null
    if (Test-CovenProviderSourceCapture -Capture $capture) { throw 'Hardlinked compilation source admitted' }
    $rejected=$false
    try { Get-CovenProviderSourceCapture -Directory $root -Commit ('1'*40) | Out-Null } catch { $rejected=$true }
    if (-not $rejected) { throw 'Hardlinked source captured as single-owned input' }
    Remove-Item -LiteralPath $alias
    $missing=Join-Path $root $inventory['template']
    Remove-Item -LiteralPath $missing
    $rejected=$false
    try { Get-CovenProviderSourceCapture -Directory $root -Commit ('1'*40) | Out-Null } catch { $rejected=$true }
    if (-not $rejected) { throw 'Missing source admitted' }
} finally { Remove-Item -LiteralPath $root -Recurse -Force }
'provider captured-source/compiled-bytes controls PASS (no native API invocation)'

# Actual portable managed observations, plus synthetic mismatch controls. No DLL load.
$managed=Get-CovenProviderManagedCapture
if (-not (Test-CovenProviderManagedCapture -Capture $managed)) { throw 'Unchanged selected runtime rejected' }
$managed.Files[0].Sha256='0'*64
if (Test-CovenProviderManagedCapture -Capture $managed) { throw 'Changed runtime digest admitted' }
$managed=Get-CovenProviderManagedCapture
$managed.Files.RemoveAt(0)
if (Test-CovenProviderManagedCapture -Capture $managed) { throw 'Missing runtime file admitted' }
$expectedLibrary=Join-Path ([IO.Path]::GetTempPath()) 'wevtapi.dll'
if (-not (Test-CovenProviderLoadedLibrary -ExpectedPath $expectedLibrary -Modules @() -Required $false)) { throw 'Absent pre-load module rejected' }
if (Test-CovenProviderLoadedLibrary -ExpectedPath $expectedLibrary -Modules @() -Required $true) { throw 'Absent required module admitted' }
$module=[pscustomobject]@{ModuleName='wevtapi.dll';FileName=$expectedLibrary}
if (-not (Test-CovenProviderLoadedLibrary -ExpectedPath $expectedLibrary -Modules @($module) -Required $true)) { throw 'Matching synthetic module rejected' }
if (Test-CovenProviderLoadedLibrary -ExpectedPath $expectedLibrary -Modules @($module,$module) -Required $true) { throw 'Ambiguous native module admitted' }
$foreign=[pscustomobject]@{ModuleName='wevtapi.dll';FileName=(Join-Path ([IO.Path]::GetTempPath()) 'foreign/wevtapi.dll')}
if (Test-CovenProviderLoadedLibrary -ExpectedPath $expectedLibrary -Modules @($foreign) -Required $false) { throw 'Foreign native module admitted' }
if (-not $IsWindows) {
    $rejected=$false
    try { Open-CovenProviderRuntimeBinding | Out-Null } catch { $rejected=$true }
    if (-not $rejected) { throw 'Unsupported native platform admitted' }
}
'provider runtime identity/mismatch/platform controls PASS (no native API invocation)'
# Exercise before/after file checking using a private stand-in file and controlled
# module observer. This does not load that file or manufacture native evidence.
$standIn=Join-Path ([IO.Path]::GetTempPath()) ('coven-library-control-'+[Guid]::NewGuid().ToString('N'))
$savedObserver=${function:Assert-CovenProviderLoadedLibrary}
try {
    function Assert-CovenProviderLoadedLibrary { param([string]$Path,[bool]$Required) }
    [IO.File]::WriteAllText($standIn,'controlled native identity')
    $fake=[pscustomobject]@{Managed=(Get-CovenProviderManagedCapture);LibraryPath=$standIn;LibrarySha256=(Get-FileHash -LiteralPath $standIn -Algorithm SHA256).Hash.ToLowerInvariant();Handle=[IntPtr]1;Closed=$false}
    if (-not (Test-CovenProviderRuntimeBinding -Binding $fake)) { throw 'Unchanged controlled library rejected' }
    [IO.File]::AppendAllText($standIn,' changed')
    if (Test-CovenProviderRuntimeBinding -Binding $fake) { throw 'Changed native library digest admitted' }
    $fake.LibrarySha256=(Get-FileHash -LiteralPath $standIn -Algorithm SHA256).Hash.ToLowerInvariant()
    $fake.Managed.Files[1].Version[0]++
    if (Test-CovenProviderRuntimeBinding -Binding $fake) { throw 'Changed selected assembly version admitted' }
} finally {
    Set-Item -Path function:Assert-CovenProviderLoadedLibrary -Value $savedObserver
    if ([IO.File]::Exists($standIn)) { [IO.File]::Delete($standIn) }
}
'provider controlled DLL digest/version mutation checks PASS (no DLL loaded)'
if (-not (Get-Command Get-CovenProviderSystemLibraryPath -ErrorAction SilentlyContinue)) { throw 'Pre-load absolute system path gate missing' }
foreach ($directory in @('', 'relative-system-directory')) {
    $rejected=$false
    try { Get-CovenProviderSystemLibraryPath -SystemDirectory $directory | Out-Null } catch { $rejected=$true }
    if (-not $rejected) { throw 'Non-absolute system library path admitted before loading' }
}
$savedRelease=if (Test-Path function:Remove-CovenProviderNativeReference) { ${function:Remove-CovenProviderNativeReference} } else { $null }
try {
    function Remove-CovenProviderNativeReference { param([IntPtr]$Handle) throw 'PRIVATE_RELEASE_FAILURE' }
    $owned=[pscustomobject]@{Handle=[IntPtr]1;Closed=$false;ReleaseAttempted=$false;ReleaseSucceeded=$false;ReleaseFailed=$false}
    $message=''
    try { Close-CovenProviderRuntimeBinding -Binding $owned } catch { $message=$_.Exception.Message }
    if (-not $owned.ReleaseAttempted -or -not $owned.ReleaseFailed -or $owned.ReleaseSucceeded -or $owned.Closed -or $message -cne 'Provider runtime reference release failed') { throw 'Release failure state lost or private diagnostic exposed' }
} finally {
    if ($null -ne $savedRelease) { Set-Item -Path function:Remove-CovenProviderNativeReference -Value $savedRelease }
    else { Remove-Item function:Remove-CovenProviderNativeReference }
}
'provider pre-load absolute path and release-failure controls PASS (no native reference released)'
$savedObserver=${function:Assert-CovenProviderLoadedLibrary}
$standIn=Join-Path ([IO.Path]::GetTempPath()) ('coven-release-state-'+[Guid]::NewGuid().ToString('N'))
try {
    function Assert-CovenProviderLoadedLibrary { param([string]$Path,[bool]$Required) }
    [IO.File]::WriteAllText($standIn,'controlled library')
    $failedRelease=[pscustomobject]@{Managed=(Get-CovenProviderManagedCapture);LibraryPath=$standIn;LibrarySha256=(Get-FileHash -LiteralPath $standIn -Algorithm SHA256).Hash.ToLowerInvariant();Handle=[IntPtr]1;Closed=$false;ReleaseAttempted=$true;ReleaseSucceeded=$false;ReleaseFailed=$true}
    if (Test-CovenProviderRuntimeBinding -Binding $failedRelease) { throw 'Failed owned release still admitted host proof' }
    $failure=New-CovenProviderBindingFailure -Binding $failedRelease
    if ($failure.Message -cne 'Provider runtime binding failed' -or $failure.Data['ProviderReleaseFailed'] -ne $true) { throw 'Secondary release failure lost from primary binding outcome' }
} finally {
    Set-Item -Path function:Assert-CovenProviderLoadedLibrary -Value $savedObserver
    if ([IO.File]::Exists($standIn)) { [IO.File]::Delete($standIn) }
}
'provider failed-release proof withholding controls PASS'

. (Join-Path $PSScriptRoot 'query-summary.ps1')
$phaseNames=@('StartCallEnteredTicks','StartCallReturnedTicks','StdinCloseEnteredTicks','StdinCloseReturnedTicks','ReaderSetupEnteredTicks','ReaderSetupReturnedTicks','MonitorEnteredTicks','TimeoutObservedTicks','CleanupEnteredTicks','KillCallEnteredTicks','KillCallReturnedTicks','RootWaitEnteredTicks','RootWaitReturnedTicks','InitialReaderWaitEnteredTicks','InitialReaderWaitReturnedTicks','FallbackReaderWaitEnteredTicks','FallbackReaderWaitReturnedTicks','ReaderDisposalAttemptsEnteredTicks','ReaderDisposalAttemptsFinishedTicks','CleanupFinishedTicks')
$phase=[pscustomobject]@{Frequency=1000L}
foreach ($name in $phaseNames) { $phase | Add-Member NoteProperty $name $null }
$phase.StartCallEnteredTicks=0L
$raw=[pscustomobject]@{Started=$true;TimedOut=$false;OutputLimited=$false;RootExited=$true;ReadersSettled=$true;Reaped=$true;ExitCode=0;ElapsedMs=12L;Phases=$phase;Stdout='PRIVATE_SENTINEL';Stderr='';Failure='';CleanupFailure='';ReaderFailure=''}
$oldOutputs=foreach ($id in @('xperf-start-help','xperf-providers-help','xperf-loggers','xperf-providers','xperf-profint','xperf-processing-help','xperf-dumper-help','xperf-tracestats-help','xperf-cswitch-help','kernel-file-schema','kernel-file-schema-no-messages','token-groups','token-privileges')) {
    ConvertTo-CovenQuerySummary -Id $id -Result $raw | ConvertTo-Json -Depth 6 -Compress
}
$oldBytes=[Text.Encoding]::UTF8.GetBytes($oldOutputs -join "`n")
$hash=Get-CovenProviderSourceHash -Bytes $oldBytes
if ($hash -cne '961fa15a77f49a620f93ab3b4a4ed8e8ba8475a45f12fb829da725e3bfeeb225') { throw 'Existing fixed-ID query summary bytes changed' }
if (-not (Get-Command ConvertTo-CovenProviderQuerySummary -ErrorAction SilentlyContinue)) { throw 'Distinct provider query summary missing' }
$summary=ConvertTo-CovenProviderQuerySummary -Result $raw
if ($summary.kind -cne 'kernel-file-metadata' -or $summary.stdoutDigestScope -cne 'decoded-text-utf8' -or $null -ne $summary.PSObject.Properties['id']) { throw 'Provider query reused external-tool identity' }
if ($summary.stdoutSha256 -cne ((ConvertTo-CovenQuerySummary -Id 'kernel-file-schema' -Result $raw).stdoutSha256) -or $summary.phases.PSObject.Properties.Name.Count -ne 22) { throw 'Provider observation semantics diverged' }
if (($summary | ConvertTo-Json -Depth 6 -Compress) -match 'PRIVATE_SENTINEL') { throw 'Provider query leaked raw output' }
'provider query projection and old fixed-ID golden bytes PASS'
