$ErrorActionPreference='Stop'
$projector=Join-Path $PSScriptRoot 'provider-template.cs'
if (-not (Test-Path -LiteralPath $projector)) { throw 'Provider template projector missing' }
Add-Type -Path $projector
function Assert-Template($Xml,[string]$Status,[int]$FieldCount,[int]$Remaining=2048) {
    $argument=if ($null -eq $Xml) { [System.Management.Automation.Language.NullString]::Value } else { $Xml }
    $result=[CovenKernelTemplate]::Project($argument,$Remaining)
    if ($result.Status -cne $Status -or $result.Fields.Count -ne $FieldCount) { throw ('Unexpected template projection: expectedStatus={0} actualStatus={1} expectedFields={2} actualFields={3} inputNull={4} inputLength={5}' -f $Status,$result.Status,$FieldCount,$result.Fields.Count,($null -eq $Xml),$Xml.Length) }
    $json=$result | ConvertTo-Json -Depth 8 -Compress
    if ($json -match 'PRIVATE_SENTINEL|/Users/private|S-1-5-21|rawXml') { throw 'Private template metadata leaked' }
    if ($null -ne $result.Sha256 -and $result.Sha256 -cnotmatch '^[0-9a-f]{64}$') { throw 'Template digest invalid' }
    return $result
}
$valid='<template xmlns="http://schemas.microsoft.com/win/2004/08/events"><data name="Irp" inType="win:Pointer"/><data name="Status" inType="win:HexInt32"/></template>'
$t=Assert-Template $valid 'supported-flat' 2
if ($t.Fields[0].Ordinal -ne 0 -or $t.Fields[0].KnownName -cne 'Irp' -or $t.Fields[0].KnownType -cne 'win:Pointer' -or $t.Fields[1].Ordinal -ne 1 -or $t.UnsupportedCount -ne 0) { throw 'Known field schema or order lost' }
$t=Assert-Template '<template><data name="PRIVATE_SENTINEL/path" inType="PRIVATE_SENTINEL:type"/></template>' 'unsupported' 1
if ($null -ne $t.Fields[0].KnownName -or $null -ne $t.Fields[0].KnownType -or -not $t.Fields[0].Unsupported -or $t.Fields[0].NameSha256 -cnotmatch '^[0-9a-f]{64}$') { throw 'Unknown fields were admitted or lost digest evidence' }
$t=Assert-Template '<template><data name="Irp" inType="win:Pointer" count="4" length="12"/></template>' 'supported-flat' 1
if ($t.Fields[0].LiteralCount -ne 4 -or $t.Fields[0].LiteralLength -ne 12) { throw 'Literal cardinality lost' }
$t=Assert-Template '<template><data name="Irp" inType="win:Pointer" count="PRIVATE_SENTINEL"/></template>' 'unsupported' 1
if ($null -ne $t.Fields[0].LiteralCount) { throw 'Dynamic cardinality was invented' }
$null=Assert-Template '<template><struct name="PRIVATE_SENTINEL"><data name="Irp" inType="win:Pointer"/></struct></template>' 'unsupported' 0
$null=Assert-Template '<template><UserData>PRIVATE_SENTINEL</UserData></template>' 'unsupported' 0
$null=Assert-Template '<wrong-root>PRIVATE_SENTINEL</wrong-root>' 'unsupported' 0
$null=Assert-Template '<template><data name="Irp" inType="win:Pointer"/><data name="Irp" inType="win:Pointer"/></template>' 'unsupported' 2
$null=Assert-Template '<template><data' 'invalid' 0
$null=Assert-Template '<!DOCTYPE template [<!ENTITY x SYSTEM "file:///PRIVATE_SENTINEL">]><template>&x;</template>' 'invalid' 0
$null=Assert-Template '' 'empty' 0
$null=Assert-Template $null 'missing' 0
$null=Assert-Template ('x'*32769) 'limit' 0
$many='<template>'+('<data name="Irp" inType="win:Pointer"/>'*129)+'</template>'
$null=Assert-Template $many 'limit' 0
$null=Assert-Template $valid 'limit' 0 1
$deep='<template>'+('<struct>'*17)+('</struct>'*17)+'</template>'
$null=Assert-Template $deep 'limit' 0
$null=Assert-Template '<template xmlns="PRIVATE_SENTINEL"><data name="Irp" inType="win:Pointer"/></template>' 'unsupported' 0
$null=Assert-Template '<template xmlns:win="PRIVATE_SENTINEL"><data name="Irp" inType="win:Pointer"/></template>' 'unsupported' 1
$null=Assert-Template '<template><data xmlns:win="PRIVATE_SENTINEL" name="Irp" inType="win:Pointer"/></template>' 'unsupported' 1
$null=Assert-Template '<template xmlns:win="http://manifests.microsoft.com/win/2004/08/windows/events"><data name="Irp" inType="win:Pointer"/></template>' 'supported-flat' 1
$comments='<template>' +('<!--x-->'*2049)+'</template>'
$null=Assert-Template $comments 'limit' 0
'provider template controlled privacy/bounds PASS (no native API invocation)'
