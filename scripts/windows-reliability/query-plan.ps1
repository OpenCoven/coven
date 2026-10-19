# Fixed read-only commands. No caller-supplied command or argument expansion.
function Get-CovenReadOnlyQueryPlan {
    foreach ($entry in @(
        @('xperf-start-help','-help','start'),
        @('xperf-providers-help','-help','providers'),
        @('xperf-loggers','-loggers'),
        @('xperf-providers','-providers'),
        @('xperf-profint','-profint'),
        @('xperf-processing-help','-help','processing'),
        @('xperf-dumper-help','-help','dumper'),
        @('xperf-tracestats-help','-help','tracestats'),
        @('xperf-cswitch-help','-help','cswitch')
    )) {
        [pscustomobject]@{ id=$entry[0]; tool='xperf'; arguments=[string[]]$entry[1..($entry.Length-1)] }
    }
    [pscustomobject]@{ id='kernel-file-schema'; tool='wevtutil'; arguments=[string[]]@('gp','Microsoft-Windows-Kernel-File','/ge:true','/gm:true','/f:xml') }
    [pscustomobject]@{ id='kernel-file-schema-no-messages'; tool='wevtutil'; arguments=[string[]]@('gp','Microsoft-Windows-Kernel-File','/ge:true','/gm:false','/f:xml') }
    [pscustomobject]@{ id='token-groups'; tool='whoami'; arguments=[string[]]@('/groups','/fo','csv') }
    [pscustomobject]@{ id='token-privileges'; tool='whoami'; arguments=[string[]]@('/priv','/fo','csv') }
}
