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

# Characterization: every legacy member of each projection, recorded from the
# projector before rejection reasons existed. Digests are shown as the input
# text they hash. Views over 1000 characters are compared by digest.
$p='http://schemas.microsoft.com/win/2004/08/events'
$w='http://manifests.microsoft.com/win/2004/08/windows/events'
$irp='<data name="Irp" inType="win:Pointer"/>'
function Get-DeepTemplate([int]$Comments,[int]$Depth) { '<template>'+('<!--c-->'*$Comments)+('<struct>'*$Depth)+('</struct>'*$Depth)+'</template>' }
$all12='<template tid="1" bad="1"><struct/><x:data xmlns:x="urn:private" name="Irp" inType="win:Pointer"/><data name="Irp" inType="win:Pointer"><map/></data>TEXT'+
    '<data name="UNKNOWN" inType="win:Pointer"/><data name="Status" inType="UNKNOWN"/><data name="Irp" inType="win:Pointer"/>'+
    '<data xmlns:win="urn:private" name="FileKey" inType="win:Pointer"/><data name="ThreadId" inType="win:UInt32" count="x"/>'+
    '<data name="IoSize" inType="win:UInt32" length="x"/><data name="IoFlags" inType="win:UInt32" map="x"/></template>'
$TemplateCorpus=@(
    @{Name='supported-two';Xml="<template xmlns=`"$p`">$irp<data name=`"Status`" inType=`"win:HexInt32`"/></template>"}
    @{Name='unknown-name-and-type';Xml='<template><data name="PRIVATE_SENTINEL/path" inType="PRIVATE_SENTINEL:type"/></template>'}
    @{Name='literal-cardinality';Xml='<template><data name="Irp" inType="win:Pointer" count="4" length="12"/></template>'}
    @{Name='cardinality-bounds';Xml='<template><data name="Irp" inType="win:Pointer" count="65535" length="0"/><data name="Status" inType="win:UInt32" count="65536"/><data name="FileKey" inType="win:Pointer" length="+1"/><data name="TTID" inType="win:UInt32" count=" 1"/><data name="IoSize" inType="win:UInt32" length=""/></template>'}
    @{Name='dynamic-count';Xml='<template><data name="Irp" inType="win:Pointer" count="PRIVATE_SENTINEL"/></template>'}
    @{Name='dynamic-length';Xml='<template><data name="Irp" inType="win:Pointer" length="PRIVATE_SENTINEL"/></template>'}
    @{Name='struct-nesting';Xml='<template><struct name="PRIVATE_SENTINEL"><data name="Irp" inType="win:Pointer"/></struct></template>'}
    @{Name='userdata-text';Xml='<template><UserData>PRIVATE_SENTINEL</UserData></template>'}
    @{Name='wrong-root-text';Xml='<wrong-root>PRIVATE_SENTINEL</wrong-root>'}
    @{Name='wrong-root-and-namespace';Xml='<wrong xmlns="urn:private"/>'}
    @{Name='wrong-root-with-data';Xml="<wrong-root>$irp</wrong-root>"}
    @{Name='root-namespace';Xml="<template xmlns=`"urn:private`">$irp</template>"}
    @{Name='root-tid';Xml="<template tid=`"t1`">$irp</template>"}
    @{Name='root-attribute';Xml="<template tid=`"t1`" other=`"PRIVATE_SENTINEL`">$irp</template>"}
    @{Name='root-namespaced-attribute';Xml="<template xmlns:q=`"urn:private`" q:tid=`"1`">$irp</template>"}
    @{Name='duplicate';Xml="<template>$irp$irp</template>"}
    @{Name='unknown-type-then-known-type-same-name';Xml="<template><data name=`"Irp`" inType=`"PRIVATE_SENTINEL`"/>$irp</template>"}
    @{Name='unknown-name-repeated';Xml='<template><data name="PRIVATE_SENTINEL" inType="win:Pointer"/><data name="PRIVATE_SENTINEL" inType="win:Pointer"/></template>'}
    @{Name='conflicting-binding-then-duplicate';Xml="<template><data xmlns:win=`"urn:private`" name=`"Irp`" inType=`"win:Pointer`"/>$irp</template>"}
    @{Name='root-conflicting-binding';Xml="<template xmlns:win=`"urn:private`">$irp</template>"}
    @{Name='field-conflicting-binding';Xml="<template><data xmlns:win=`"urn:private`" name=`"Irp`" inType=`"win:Pointer`"/></template>"}
    @{Name='manifest-binding';Xml="<template xmlns:win=`"$w`">$irp</template>"}
    @{Name='multiple-defects';Xml='<template><data name="PRIVATE_SENTINEL" inType="PRIVATE_SENTINEL" count="x" length="y" map="z"/></template>'}
    @{Name='field-extra-attribute';Xml='<template><data name="Irp" inType="win:Pointer" map="PRIVATE_SENTINEL"/></template>'}
    @{Name='field-namespaced-attribute';Xml='<template><data xmlns:q="urn:private" q:map="1" name="Irp" inType="win:Pointer"/></template>'}
    @{Name='empty-data';Xml='<template><data/></template>'}
    @{Name='data-namespace';Xml='<template><x:data xmlns:x="urn:private" name="Irp" inType="win:Pointer"/></template>'}
    @{Name='nested-in-data';Xml='<template><data name="Irp" inType="win:Pointer"><map/></data></template>'}
    @{Name='nested-in-unsupported-data';Xml='<template><data name="PRIVATE_SENTINEL" inType="win:Pointer"><map/></data></template>'}
    @{Name='text-in-data';Xml='<template><data name="Irp" inType="win:Pointer">PRIVATE_SENTINEL</data></template>'}
    @{Name='nested-then-text-in-data';Xml='<template><data name="Irp" inType="win:Pointer"><map/>PRIVATE_SENTINEL</data></template>'}
    @{Name='text-after-data-end';Xml='<template><data name="Irp" inType="win:Pointer"></data>PRIVATE_SENTINEL</template>'}
    @{Name='text-before-data';Xml="<template>PRIVATE_SENTINEL$irp</template>"}
    @{Name='cdata';Xml='<template><![CDATA[PRIVATE_SENTINEL]]></template>'}
    @{Name='whitespace-comments-pi';Xml="<template> <!--c--><?pi x?>$irp `n</template>"}
    @{Name='all-twelve';Xml=$all12}
    @{Name='invalid-xml';Xml='<template><data'}
    @{Name='dtd';Xml='<!DOCTYPE template [<!ENTITY x SYSTEM "file:///PRIVATE_SENTINEL">]><template>&x;</template>'}
    @{Name='second-root';Xml="<template>$irp</template><template/>"}
    @{Name='unpaired-surrogate';Xml=('<template>'+[string][char]0xD800+'</template>')}
    @{Name='empty';Xml=''}
    @{Name='missing';Xml=$null}
    @{Name='size-limit';Xml=('x'*32769)}
    @{Name='size-boundary';Xml=('<template>'+(' '*(32768-21))+'</template>')}
    @{Name='field-limit-after-reasons';Xml=('<template><data name="PRIVATE_SENTINEL" inType="win:Pointer"/>'+($irp*128)+'</template>')}
    @{Name='field-limit-boundary';Xml=('<template>'+('<data name="Irp" inType="win:Pointer"/>'*128)+'</template>')}
    @{Name='field-limit-before-budget';Xml=('<template>'+($irp*129)+'</template>');Remaining=128}
    @{Name='field-budget';Xml=('<template><struct/>'+($irp*6)+'</template>');Remaining=5}
    @{Name='field-budget-zero';Xml="<template>$irp</template>";Remaining=0}
    @{Name='field-budget-boundary';Xml="<template>$irp$irp</template>";Remaining=2}
    @{Name='depth-limit-after-reasons';Xml=(Get-DeepTemplate 0 17)}
    @{Name='depth-boundary';Xml=(Get-DeepTemplate 0 16)}
    @{Name='node-limit-comments';Xml=('<template>'+('<!--x-->'*2049)+'</template>')}
    @{Name='node-limit-before-depth';Xml=(Get-DeepTemplate 2031 17)}
    @{Name='depth-limit-at-node-boundary';Xml=(Get-DeepTemplate 2030 17)}
)
$TemplateUtf8=[Text.UTF8Encoding]::new($false,$true)
function Get-TemplateTestDigest([string]$Text) { [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($TemplateUtf8.GetBytes($Text))).ToLowerInvariant() }
# Every legacy member, with each digest shown as the input text it hashes.
function Get-LegacyTemplateView($Xml,$Result) {
    $labels=@{(Get-TemplateTestDigest '')='""'}
    if ($null -ne $Result.Sha256) {
        $labels[(Get-TemplateTestDigest $Xml)]='xml'
        foreach ($m in [regex]::Matches($Xml,'(?:name|inType)="([^"]*)"')) { $labels[(Get-TemplateTestDigest $m.Groups[1].Value)]='"'+$m.Groups[1].Value+'"' }
    }
    $label={param($d) if ($null -eq $d) { 'null' } elseif ($labels.ContainsKey($d)) { $labels[$d] } else { 'unmatched:'+$d } }
    $fields=foreach ($f in $Result.Fields) {
        '{0}:{1}={2}:{3}={4}:{5}:{6}:{7}' -f $f.Ordinal,$(if($null -eq $f.KnownName){'?'}else{$f.KnownName}),(& $label $f.NameSha256),
            $(if($null -eq $f.KnownType){'?'}else{$f.KnownType}),(& $label $f.TypeSha256),
            $(if($null -eq $f.LiteralCount){'-'}else{$f.LiteralCount}),$(if($null -eq $f.LiteralLength){'-'}else{$f.LiteralLength}),$(if($f.Unsupported){'U'}else{'S'})
    }
    '{0} units={1} sha={2} unsupported={3} fields=[{4}]' -f $Result.Status,$(if($null -eq $Result.Utf16CodeUnits){'null'}else{$Result.Utf16CodeUnits}),(& $label $Result.Sha256),$Result.UnsupportedCount,(@($fields) -join ' ')
}
function Invoke-TemplateCorpus([type]$Projector) {
    $views=[ordered]@{}
    foreach ($case in $TemplateCorpus) {
        $remaining=if ($case.ContainsKey('Remaining')) { $case.Remaining } else { 2048 }
        $argument=if ($null -eq $case.Xml) { [System.Management.Automation.Language.NullString]::Value } else { $case.Xml }
        $result=$Projector::Project($argument,$remaining)
        $view=Get-LegacyTemplateView $case.Xml $result
        if ($view.Length -gt 1000) { $view='sha256:'+(Get-TemplateTestDigest $view) }
        $views[$case.Name]=[pscustomobject]@{View=$view;Result=$result}
    }
    $views
}
$LegacyTemplateViews=[ordered]@{
    'supported-two'='supported-flat units=159 sha=xml unsupported=0 fields=[0:Irp="Irp":win:Pointer="win:Pointer":-:-:S 1:Status="Status":win:HexInt32="win:HexInt32":-:-:S]'
    'unknown-name-and-type'='unsupported units=88 sha=xml unsupported=1 fields=[0:?="PRIVATE_SENTINEL/path":?="PRIVATE_SENTINEL:type":-:-:U]'
    'literal-cardinality'='supported-flat units=82 sha=xml unsupported=0 fields=[0:Irp="Irp":win:Pointer="win:Pointer":4:12:S]'
    'cardinality-bounds'='unsupported units=296 sha=xml unsupported=4 fields=[0:Irp="Irp":win:Pointer="win:Pointer":65535:0:S 1:Status="Status":win:UInt32="win:UInt32":-:-:U 2:FileKey="FileKey":win:Pointer="win:Pointer":-:-:U 3:TTID="TTID":win:UInt32="win:UInt32":-:-:U 4:IoSize="IoSize":win:UInt32="win:UInt32":-:-:U]'
    'dynamic-count'='unsupported units=85 sha=xml unsupported=1 fields=[0:Irp="Irp":win:Pointer="win:Pointer":-:-:U]'
    'dynamic-length'='unsupported units=86 sha=xml unsupported=1 fields=[0:Irp="Irp":win:Pointer="win:Pointer":-:-:U]'
    'struct-nesting'='unsupported units=101 sha=xml unsupported=2 fields=[]'
    'userdata-text'='unsupported units=58 sha=xml unsupported=2 fields=[]'
    'wrong-root-text'='unsupported units=41 sha=xml unsupported=2 fields=[]'
    'wrong-root-and-namespace'='unsupported units=28 sha=xml unsupported=1 fields=[]'
    'wrong-root-with-data'='unsupported units=64 sha=xml unsupported=2 fields=[]'
    'root-namespace'='unsupported units=80 sha=xml unsupported=2 fields=[]'
    'root-tid'='supported-flat units=69 sha=xml unsupported=0 fields=[0:Irp="Irp":win:Pointer="win:Pointer":-:-:S]'
    'root-attribute'='unsupported units=94 sha=xml unsupported=1 fields=[0:Irp="Irp":win:Pointer="win:Pointer":-:-:S]'
    'root-namespaced-attribute'='unsupported units=92 sha=xml unsupported=1 fields=[0:Irp="Irp":win:Pointer="win:Pointer":-:-:S]'
    'duplicate'='unsupported units=99 sha=xml unsupported=1 fields=[0:Irp="Irp":win:Pointer="win:Pointer":-:-:S 1:Irp="Irp":win:Pointer="win:Pointer":-:-:U]'
    'unknown-type-then-known-type-same-name'='unsupported units=104 sha=xml unsupported=1 fields=[0:Irp="Irp":?="PRIVATE_SENTINEL":-:-:U 1:Irp="Irp":win:Pointer="win:Pointer":-:-:S]'
    'unknown-name-repeated'='unsupported units=125 sha=xml unsupported=2 fields=[0:?="PRIVATE_SENTINEL":win:Pointer="win:Pointer":-:-:U 1:?="PRIVATE_SENTINEL":win:Pointer="win:Pointer":-:-:U]'
    'conflicting-binding-then-duplicate'='unsupported units=123 sha=xml unsupported=2 fields=[0:Irp="Irp":?="win:Pointer":-:-:U 1:Irp="Irp":win:Pointer="win:Pointer":-:-:U]'
    'root-conflicting-binding'='unsupported units=84 sha=xml unsupported=1 fields=[0:Irp="Irp":?="win:Pointer":-:-:U]'
    'field-conflicting-binding'='unsupported units=84 sha=xml unsupported=1 fields=[0:Irp="Irp":?="win:Pointer":-:-:U]'
    'manifest-binding'='supported-flat units=130 sha=xml unsupported=0 fields=[0:Irp="Irp":win:Pointer="win:Pointer":-:-:S]'
    'multiple-defects'='unsupported units=107 sha=xml unsupported=1 fields=[0:?="PRIVATE_SENTINEL":?="PRIVATE_SENTINEL":-:-:U]'
    'field-extra-attribute'='unsupported units=83 sha=xml unsupported=1 fields=[0:Irp="Irp":win:Pointer="win:Pointer":-:-:U]'
    'field-namespaced-attribute'='unsupported units=92 sha=xml unsupported=1 fields=[0:Irp="Irp":win:Pointer="win:Pointer":-:-:U]'
    'empty-data'='unsupported units=28 sha=xml unsupported=1 fields=[0:?="":?="":-:-:U]'
    'data-namespace'='unsupported units=84 sha=xml unsupported=1 fields=[]'
    'nested-in-data'='unsupported units=72 sha=xml unsupported=1 fields=[0:Irp="Irp":win:Pointer="win:Pointer":-:-:U]'
    'nested-in-unsupported-data'='unsupported units=85 sha=xml unsupported=2 fields=[0:?="PRIVATE_SENTINEL":win:Pointer="win:Pointer":-:-:U]'
    'text-in-data'='unsupported units=82 sha=xml unsupported=1 fields=[0:Irp="Irp":win:Pointer="win:Pointer":-:-:U]'
    'nested-then-text-in-data'='unsupported units=88 sha=xml unsupported=2 fields=[0:Irp="Irp":win:Pointer="win:Pointer":-:-:U]'
    'text-after-data-end'='unsupported units=82 sha=xml unsupported=1 fields=[0:Irp="Irp":win:Pointer="win:Pointer":-:-:S]'
    'text-before-data'='unsupported units=76 sha=xml unsupported=1 fields=[0:Irp="Irp":win:Pointer="win:Pointer":-:-:S]'
    'cdata'='unsupported units=49 sha=xml unsupported=1 fields=[]'
    'whitespace-comments-pi'='supported-flat units=79 sha=xml unsupported=0 fields=[0:Irp="Irp":win:Pointer="win:Pointer":-:-:S]'
    'all-twelve'='unsupported units=506 sha=xml unsupported=12 fields=[0:Irp="Irp":win:Pointer="win:Pointer":-:-:U 1:?="UNKNOWN":win:Pointer="win:Pointer":-:-:U 2:Status="Status":?="UNKNOWN":-:-:U 3:Irp="Irp":win:Pointer="win:Pointer":-:-:U 4:FileKey="FileKey":?="win:Pointer":-:-:U 5:ThreadId="ThreadId":win:UInt32="win:UInt32":-:-:U 6:IoSize="IoSize":win:UInt32="win:UInt32":-:-:U 7:IoFlags="IoFlags":win:UInt32="win:UInt32":-:-:U]'
    'invalid-xml'='invalid units=15 sha=xml unsupported=0 fields=[]'
    'dtd'='invalid units=91 sha=xml unsupported=0 fields=[]'
    'second-root'='invalid units=71 sha=xml unsupported=0 fields=[]'
    'unpaired-surrogate'='invalid units=22 sha=null unsupported=0 fields=[]'
    'empty'='empty units=0 sha=xml unsupported=0 fields=[]'
    'missing'='missing units=null sha=null unsupported=0 fields=[]'
    'size-limit'='limit units=32769 sha=null unsupported=0 fields=[]'
    'size-boundary'='supported-flat units=32768 sha=xml unsupported=0 fields=[]'
    'field-limit-after-reasons'='limit units=5065 sha=xml unsupported=0 fields=[]'
    'field-limit-boundary'='sha256:63bb71929e0769295117efef795654442441e74fe1a30988c1e34b67b2b3fc6a'
    'field-limit-before-budget'='limit units=5052 sha=xml unsupported=0 fields=[]'
    'field-budget'='limit units=264 sha=xml unsupported=0 fields=[]'
    'field-budget-zero'='limit units=60 sha=xml unsupported=0 fields=[]'
    'field-budget-boundary'='unsupported units=99 sha=xml unsupported=1 fields=[0:Irp="Irp":win:Pointer="win:Pointer":-:-:S 1:Irp="Irp":win:Pointer="win:Pointer":-:-:U]'
    'depth-limit-after-reasons'='limit units=310 sha=xml unsupported=0 fields=[]'
    'depth-boundary'='unsupported units=293 sha=xml unsupported=16 fields=[]'
    'node-limit-comments'='limit units=16413 sha=xml unsupported=0 fields=[]'
    'node-limit-before-depth'='limit units=16558 sha=xml unsupported=0 fields=[]'
    'depth-limit-at-node-boundary'='limit units=16550 sha=xml unsupported=0 fields=[]'
}
function Get-LegacyTemplateMismatches($Views) {
    @(foreach ($name in $LegacyTemplateViews.Keys) { if ($Views[$name].View -cne $LegacyTemplateViews[$name]) { $name } })
}
$corpusViews=Invoke-TemplateCorpus ([CovenKernelTemplate])
if ($corpusViews.Count -ne $LegacyTemplateViews.Count -or $TemplateCorpus.Count -ne $LegacyTemplateViews.Count) { throw 'Characterization corpus and expectations drifted' }
$changed=Get-LegacyTemplateMismatches $corpusViews
if ($changed.Count -ne 0) { throw ('Legacy template projection changed: '+($changed -join ',')) }
foreach ($budget in @(-1,2049)) {
    $refused=$false
    try { $null=[CovenKernelTemplate]::Project($valid,$budget) } catch { $refused=$true }
    if (-not $refused) { throw 'Field budget precondition lost' }
}
'provider template controlled privacy/bounds PASS (no native API invocation)'
