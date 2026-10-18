# Only fixed identifiers, numeric accounting, booleans and digests cross this boundary.
function ConvertTo-CovenQuerySummary {
    param(
        [Parameter(Mandatory)][ValidateSet('xperf-start-help','xperf-providers-help','xperf-loggers','xperf-providers','xperf-profint','xperf-processing-help','xperf-dumper-help','xperf-tracestats-help','xperf-cswitch-help','kernel-file-schema','kernel-file-schema-no-messages','token-groups','token-privileges')][string]$Id,
        [Parameter(Mandatory)]$Result
    )
    $digest = {
        param([string]$Text)
        [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes($Text))).ToLowerInvariant()
    }
    [pscustomobject][ordered]@{
        id = $Id
        started = [bool]$Result.Started
        timedOut = [bool]$Result.TimedOut
        outputLimited = [bool]$Result.OutputLimited
        rootExited = [bool]$Result.RootExited
        readersSettled = [bool]$Result.ReadersSettled
        reaped = [bool]$Result.Reaped
        exitCode = if ($null -eq $Result.ExitCode) { $null } else { [int]$Result.ExitCode }
        elapsedMs = [long]$Result.ElapsedMs
        hasFailure = -not [string]::IsNullOrEmpty($Result.Failure)
        hasCleanupFailure = -not [string]::IsNullOrEmpty($Result.CleanupFailure)
        hasReaderFailure = -not [string]::IsNullOrEmpty($Result.ReaderFailure)
        stdoutChars = ([string]$Result.Stdout).Length
        stderrChars = ([string]$Result.Stderr).Length
        stdoutSha256 = & $digest ([string]$Result.Stdout)
        stderrSha256 = & $digest ([string]$Result.Stderr)
    }
}
