# Return object is internal. Publish ONLY .summaries/.aborted, never .rawResults.
function Invoke-CovenPreflightQueries {
    param(
        [Parameter(Mandatory)][AllowEmptyCollection()][hashtable]$Identities,
        [scriptblock]$InvokeQuery = { param($exe,$argv,$timeout,$limit) [CovenNativeQuery]::Run($exe,[string[]]$argv,$timeout,$limit) }
    )
    $summaries=[Collections.Generic.List[object]]::new()
    $rawResults=@{}
    $aborted=$false
    foreach ($query in Get-CovenReadOnlyQueryPlan) {
        $identity=$Identities[$query.tool]
        if ($aborted -or $null -eq $identity -or [string]::IsNullOrEmpty($identity.path)) {
            $summaries.Add([pscustomobject]@{id=$query.id;available=($null -ne $identity -and -not [string]::IsNullOrEmpty($identity.path));executed=$false;skippedAfterCleanupFailure=$aborted})
            continue
        }
        # Exactly one invocation per fixed query. No retry on failed query or decoding.
        try {
            $raw=& $InvokeQuery $identity.path ([string[]]$query.arguments) 10000 524288
        } catch {
            # No claim about cleanup is possible after an unexpected helper exception.
            $summaries.Add([pscustomobject]@{id=$query.id;available=$true;executed=$true;invocationThrew=$true;cleanupProven=$false})
            $aborted=$true
            continue
        }
        $rawResults[$query.id]=$raw
        $safe=ConvertTo-CovenQuerySummary -Id $query.id -Result $raw
        $safe | Add-Member -NotePropertyName available -NotePropertyValue $true
        $safe | Add-Member -NotePropertyName executed -NotePropertyValue $true
        $summaries.Add($safe)
        if (-not $raw.Reaped -or -not $raw.RootExited -or -not $raw.ReadersSettled -or
            -not [string]::IsNullOrEmpty($raw.CleanupFailure)) { $aborted=$true }
    }
    [pscustomobject]@{summaries=$summaries.ToArray();aborted=$aborted;rawResults=$rawResults}
}
