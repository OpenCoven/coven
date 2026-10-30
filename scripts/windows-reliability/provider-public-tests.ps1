param([string]$OutputFile,[string]$ProbeOutputFile)
$ErrorActionPreference='Stop'
$public=Join-Path $PSScriptRoot 'provider-public.cs'
if (-not (Test-Path -LiteralPath $public)) { throw 'Bounded collector encoder missing' }
$references=@(Get-ChildItem -LiteralPath (Join-Path $PSHOME 'ref') -Filter '*.dll' | ForEach-Object FullName)
Add-Type -Path @((Join-Path $PSScriptRoot 'provider-template.cs'),(Join-Path $PSScriptRoot 'provider-metadata-reader.cs'),(Join-Path $PSScriptRoot 'provider-reader-tests.cs'),$public,(Join-Path $PSScriptRoot 'provider-public-tests.cs')) -ReferencedAssemblies $references
[ControlledPublicTests]::Execute()
[ControlledPublicTests]::ExecuteProbe()

if ($OutputFile) {
    $records=[Collections.Generic.List[object]]::new()
    foreach ($scenario in @(@{Count=1;Fault=''},@{Count=257;Fault='global-fields'},@{Count=257;Fault='eight-fields'},@{Count=256;Fault='eight-fields'},@{Count=1;Fault='primary-and-cleanup'},@{Count=1;Fault='private'})) {
        $api=[ControlledKernelApi]::new();$api.EventCount=$scenario.Count;$api.Fault=$scenario.Fault
        $collector=[CovenKernelReader]::Collect($api,[ControlledKernelBuffers]::new())
        $encoded=[CovenKernelPublic]::Encode($collector,[CovenKernelJsonSerializer]::new())
        $records.Add(@{exitCode=$encoded.ExitCode;payloadBase64=[Convert]::ToBase64String($encoded.Payload);originalOperation=$collector.Operation.ErrorClass;originalCleanupComplete=$collector.Cleanup.Complete})
    }
    foreach ($both in @($false,$true)) {
        $api=[ControlledKernelApi]::new()
        $collector=[CovenKernelReader]::Collect($api,[ControlledKernelBuffers]::new())
        $serializer=[ControlledSerializer]::new();$serializer.FirstThrows=$true;$serializer.FallbackThrows=$both;$serializer.PartialPrefix=$true
        $encoded=[CovenKernelPublic]::Encode($collector,$serializer)
        $records.Add(@{exitCode=$encoded.ExitCode;payloadBase64=[Convert]::ToBase64String($encoded.Payload);originalOperation=$collector.Operation.ErrorClass;originalCleanupComplete=$collector.Cleanup.Complete})
    }
    [IO.File]::WriteAllText($OutputFile,($records | ConvertTo-Json -Depth 8 -Compress),[Text.UTF8Encoding]::new($false))
}

if ($ProbeOutputFile) {
    $records=[Collections.Generic.List[object]]::new()
    $phaseHash=(Get-FileHash -LiteralPath (Join-Path $PSScriptRoot 'public_receipt.py') -Algorithm SHA256).Hash.ToLowerInvariant()
    foreach ($scenario in @(@{Count=1;Fault=''},@{Count=257;Fault='global-fields'},@{Count=257;Fault='eight-fields'},@{Count=256;Fault='eight-fields'},@{Count=1;Fault='private'},@{Count=2;Fault='eight-fields'})) {
        $api=[ControlledKernelApi]::new();$api.EventCount=$scenario.Count;$api.Fault=$scenario.Fault
        $collector=[CovenKernelReader]::Collect($api,[ControlledKernelBuffers]::new())
        $source=[ControlledPublicTests]::Source($phaseHash);$hostProof=[ControlledPublicTests]::Host()
        $encoded=[CovenKernelPublic]::EncodeProbe($collector,$source,$hostProof,[CovenKernelJsonSerializer]::new())
        $records.Add(@{exitCode=$encoded.ExitCode;payloadBase64=[Convert]::ToBase64String($encoded.Payload);originalOperation=$collector.Operation.ErrorClass})
    }
    [IO.File]::WriteAllText($ProbeOutputFile,($records | ConvertTo-Json -Depth 8 -Compress),[Text.UTF8Encoding]::new($false))
}
