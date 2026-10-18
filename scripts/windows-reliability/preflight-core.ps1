# Internal result separates public receipt from raw query data. Never serialize the whole result.
function Invoke-CovenNativeMetadataPreflight {
    param(
        [bool]$WindowsHost = [OperatingSystem]::IsWindows(),
        [scriptblock]$Discover = { Get-CovenInstalledToolIdentities },
        [scriptblock]$InvokeQuery = { param($exe,$argv,$timeout,$limit) [CovenNativeQuery]::Run($exe,[string[]]$argv,$timeout,$limit) }
    )
    if (-not $WindowsHost) { throw 'Windows host required' }
    $tools=& $Discover
    $queries=Invoke-CovenPreflightQueries -Identities $tools -InvokeQuery $InvokeQuery
    $schemas=[Collections.Generic.List[object]]::new()
    foreach ($id in @('kernel-file-schema','kernel-file-schema-no-messages')) {
        $raw=$queries.rawResults[$id]
        if ($null -ne $raw -and $raw.Started -and $raw.ExitCode -eq 0 -and
            $raw.Reaped -and -not $raw.TimedOut -and -not $raw.OutputLimited -and
            [string]::IsNullOrEmpty($raw.Failure) -and [string]::IsNullOrEmpty($raw.ReaderFailure) -and
            [string]::IsNullOrEmpty($raw.CleanupFailure)) {
            $schemas.Add([pscustomobject]@{id=$id;parsed=$true;schema=(ConvertTo-CovenKernelFileSchema -Xml $raw.Stdout)})
        } else {
            $schemas.Add([pscustomobject]@{id=$id;parsed=$false})
        }
    }
    $before=[Diagnostics.Stopwatch]::GetTimestamp()
    $utc=[DateTime]::UtcNow.ToString('O')
    $after=[Diagnostics.Stopwatch]::GetTimestamp()
    [pscustomobject]@{
        public=[pscustomobject][ordered]@{
            formatVersion=1
            captureAttempted=$false
            metadataComplete=$false
            # These omissions are explicit; hashes alone do not establish trace admissibility.
            pending=@('provider-schema-decoding','token-summary','controller-capability-decoding','outer-workflow-guard','native-validation')
            tools=@($tools.Keys | Sort-Object | ForEach-Object { $tools[$_].summary })
            providerSchemas=$schemas.ToArray()
            queries=$queries.summaries
            aborted=$queries.aborted
            clock=[pscustomobject]@{isHighResolution=[Diagnostics.Stopwatch]::IsHighResolution;frequency=[Diagnostics.Stopwatch]::Frequency;before=$before;utc=$utc;after=$after}
        }
        rawResults=$queries.rawResults
    }
}
