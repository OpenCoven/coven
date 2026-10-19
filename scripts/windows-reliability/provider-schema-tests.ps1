$ErrorActionPreference='Stop'
. "$PSScriptRoot/provider-schema.ps1"
$x='<publisherMetadata name="Microsoft-Windows-Kernel-File" guid="{EDD08927-9CC4-4E65-B970-C2560FB5C289}"><resourceFileName>SECRET private path</resourceFileName><events><event value="21" version="1"><template><data name="Irp" inType="win:Pointer"/><data name="SECRET" inType="win:UnicodeString"/></template></event><event value="24" version="0"><template><data name="Status" inType="win:UInt32"/></template></event></events></publisherMetadata>'
$r=ConvertTo-CovenKernelFileSchema -Xml $x
if (-not $r.providerMatched -or $r.events.Count -ne 2 -or $r.events[0].id -ne 21 -or $r.events[0].fields[0].name -ne 'Irp') { throw 'Schema fields lost' }
if (($r | ConvertTo-Json -Depth 12) -match 'SECRET|private path|resourceFileName') { throw 'Metadata string leaked' }
if ($r.events[0].fields[1].name -ne $null -or $r.events[0].fields[1].nameSha256 -notmatch '^[a-f0-9]{64}$') { throw 'Unknown field not accounted' }
foreach ($bad in @('<notxml',($x.Replace('Microsoft-Windows-Kernel-File','Other')),('<!DOCTYPE x [<!ENTITY y SYSTEM "file:///etc/passwd">]><publisherMetadata>&y;</publisherMetadata>'))) {
 $v=ConvertTo-CovenKernelFileSchema -Xml $bad
 if ($v.providerMatched -or $v.events.Count -ne 0) { throw 'Untrusted schema accepted' }
}
'provider schema privacy PASS'
