# Only fixed identifiers, numeric accounting, booleans and digests cross this boundary.
function ConvertTo-CovenQueryObservation {
    param([Parameter(Mandatory)]$Result)
    # Copy only the closed numeric observation surface, never the private object.
    $phases = [ordered]@{ unit='stopwatch-elapsed-ticks'; frequency=$null }
    $names = @('StartCallEnteredTicks','StartCallReturnedTicks','StdinCloseEnteredTicks','StdinCloseReturnedTicks',
        'ReaderSetupEnteredTicks','ReaderSetupReturnedTicks','MonitorEnteredTicks','TimeoutObservedTicks','CleanupEnteredTicks',
        'KillCallEnteredTicks','KillCallReturnedTicks','RootWaitEnteredTicks','RootWaitReturnedTicks',
        'InitialReaderWaitEnteredTicks','InitialReaderWaitReturnedTicks','FallbackReaderWaitEnteredTicks','FallbackReaderWaitReturnedTicks',
        'ReaderDisposalAttemptsEnteredTicks','ReaderDisposalAttemptsFinishedTicks','CleanupFinishedTicks')
    try {
        if ($null -eq $Result.Phases) { throw 'missing' }
        $frequency = $Result.Phases.Frequency
        if (($frequency -isnot [long] -and $frequency -isnot [int]) -or $frequency -le 0) { throw 'frequency' }
        $phases.frequency = [long]$frequency
        foreach ($name in $names) {
            $property = $Result.Phases.PSObject.Properties[$name]
            if ($null -eq $property) { throw 'missing' }
            $value = $property.Value
            if ($null -ne $value -and (($value -isnot [long] -and $value -isnot [int]) -or $value -lt 0)) { throw 'tick' }
            $key = [char]::ToLowerInvariant($name[0]) + $name.Substring(1)
            $phases[$key] = if ($null -eq $value) { $null } else { [long]$value }
        }
    } catch { throw 'Invalid query phase observations' }
    $digest = {
        param([string]$Text)
        [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes($Text))).ToLowerInvariant()
    }
    [pscustomobject][ordered]@{
        started = [bool]$Result.Started
        timedOut = [bool]$Result.TimedOut
        outputLimited = [bool]$Result.OutputLimited
        rootExited = [bool]$Result.RootExited
        readersSettled = [bool]$Result.ReadersSettled
        reaped = [bool]$Result.Reaped
        exitCode = if ($null -eq $Result.ExitCode) { $null } else { [int]$Result.ExitCode }
        elapsedMs = [long]$Result.ElapsedMs
        phases = [pscustomobject]$phases
        hasFailure = -not [string]::IsNullOrEmpty($Result.Failure)
        hasCleanupFailure = -not [string]::IsNullOrEmpty($Result.CleanupFailure)
        hasReaderFailure = -not [string]::IsNullOrEmpty($Result.ReaderFailure)
        stdoutChars = ([string]$Result.Stdout).Length
        stderrChars = ([string]$Result.Stderr).Length
        stdoutSha256 = & $digest ([string]$Result.Stdout)
        stderrSha256 = & $digest ([string]$Result.Stderr)
    }
}

function ConvertTo-CovenQuerySummary {
    param(
        [Parameter(Mandatory)][ValidateSet('xperf-start-help','xperf-providers-help','xperf-loggers','xperf-providers','xperf-profint','xperf-processing-help','xperf-dumper-help','xperf-tracestats-help','xperf-cswitch-help','kernel-file-schema','kernel-file-schema-no-messages','token-groups','token-privileges')][string]$Id,
        [Parameter(Mandatory)]$Result
    )
    $observation=ConvertTo-CovenQueryObservation -Result $Result
    $summary=[ordered]@{id=$Id}
    foreach ($property in $observation.PSObject.Properties) { $summary[$property.Name]=$property.Value }
    [pscustomobject]$summary
}
function ConvertTo-CovenProviderQuerySummary {
    param([Parameter(Mandatory)]$Result)
    $observation=ConvertTo-CovenQueryObservation -Result $Result
    $summary=[ordered]@{kind='kernel-file-metadata';stdoutDigestScope='decoded-text-utf8'}
    foreach ($property in $observation.PSObject.Properties) { $summary[$property.Name]=$property.Value }
    [pscustomobject]$summary
}

# A fixture observation is distinct from an executed metadata query. Projection
# failure must not replace the fixture assertion or bypass its owned teardown.
function ConvertTo-CovenCleanupObservation {
    param($Result,[string]$Fault,[Runtime.InteropServices.SafeHandle]$StdoutHandle,[Runtime.InteropServices.SafeHandle]$StderrHandle)
    $label=if ($Fault -ceq 'after-start') { 'after-start' } elseif ($Fault -ceq 'reader') { 'reader' } else { 'other' }
    try {
        $summary=ConvertTo-CovenQuerySummary -Id 'xperf-start-help' -Result $Result
        [pscustomobject][ordered]@{
            kind='cleanup-fixture'; fault=$label; projectionFailed=$false
            started=$summary.started; timedOut=$summary.timedOut; outputLimited=$summary.outputLimited
            rootExited=$summary.rootExited; readersSettled=$summary.readersSettled; reaped=$summary.reaped
            exitCode=$summary.exitCode; elapsedMs=$summary.elapsedMs; phases=$summary.phases
            hasFailure=$summary.hasFailure; hasCleanupFailure=$summary.hasCleanupFailure; hasReaderFailure=$summary.hasReaderFailure
            stdoutHandleClosed=if ($null -eq $StdoutHandle) { $null } else { [bool]$StdoutHandle.IsClosed }
            stderrHandleClosed=if ($null -eq $StderrHandle) { $null } else { [bool]$StderrHandle.IsClosed }
        }
    } catch { [pscustomobject][ordered]@{ kind='cleanup-fixture'; fault=$label; projectionFailed=$true } }
}
