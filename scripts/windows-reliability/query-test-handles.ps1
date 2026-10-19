function Get-OwnedReaderHandle([IO.StreamReader]$Reader) {
    $stream=$Reader.BaseStream
    if ($stream -is [IO.FileStream]) { return $stream.SafeFileHandle }
    if ($stream -is [IO.Pipes.PipeStream]) { return $stream.SafePipeHandle }
    throw 'Unexpected redirected stream type in handle-closure test'
}

function Clear-OwnedInheritedFixture($Readers,$Processes,$Files,[bool]$UnownedLeafPossible=$false) {
    $failures=[Collections.Generic.List[string]]::new()
    $allExited=-not $UnownedLeafPossible
    if ($UnownedLeafPossible) { $failures.Add('LeafOwnershipUnproved') }
    foreach ($reader in $Readers) {
        if ($null -eq $reader) { continue }
        try { $null=$reader.Dispose() } catch { $failures.Add('ReaderDispose:'+ $_.Exception.GetType().Name) }
    }
    foreach ($owned in $Processes) {
        if ($null -eq $owned) { continue }
        $exited=$false
        try { $exited=$owned.HasExited } catch { $failures.Add('ProcessStatus:'+ $_.Exception.GetType().Name) }
        if (-not $exited) {
            try { $null=$owned.Kill($true) } catch { $failures.Add('ProcessKill:'+ $_.Exception.GetType().Name) }
            try { $exited=$owned.WaitForExit(3000) } catch { $failures.Add('ProcessWait:'+ $_.Exception.GetType().Name) }
        }
        if ($exited) {
            try { $null=$owned.Dispose() } catch { $failures.Add('ProcessDispose:'+ $_.Exception.GetType().Name) }
        } else {
            # Retain the handle and files when termination could not be proved.
            $allExited=$false
            $failures.Add('ProcessExitUnproved')
        }
    }
    if ($allExited) {
        foreach ($file in $Files) {
            try { if (Test-Path -LiteralPath $file -ErrorAction Stop) { Remove-Item -LiteralPath $file -ErrorAction Stop } }
            catch { $failures.Add('FixtureFileCleanup:'+ $_.Exception.GetType().Name) }
        }
    }
    return [pscustomobject]@{ AllExited=$allExited; Failures=@($failures.ToArray()) }
}
