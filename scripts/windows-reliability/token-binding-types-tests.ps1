$ErrorActionPreference='Stop'
$parts=foreach($name in @('token-privileges.cs','token-reader.cs','token-native.cs','token-public.cs','token-binding-types.cs')) {
    $path=Join-Path $PSScriptRoot $name
    if(-not [IO.File]::Exists($path)){throw 'Token binding controlled source missing'}
    "namespace CovenTokenIdentityControlled {`n"+[IO.File]::ReadAllText($path,[Text.UTF8Encoding]::new($false,$true))+"`n}"
}
$parts+=@'
namespace CovenTokenIdentityControlled {
    public sealed class UnapprovedIdentity { public string Path=>"PRIVATE_TOKEN_PATH_SENTINEL"; }
    public static class TokenIdentityFixture {
        public static CovenTokenQueryResult Create()=>new CovenTokenQueryResult(CovenTokenFailure.None,0,6,0,6,0,2,0,new CovenEffectiveTokenFacts(CovenTokenContext.Process,1,null,3,false,new CovenTokenPrivilegeFacts(true,1)));
    }
}
'@
$references=@(Get-ChildItem -LiteralPath (Join-Path $PSHOME 'ref') -Filter '*.dll' | ForEach-Object FullName)
Add-Type -TypeDefinition ($parts -join "`n") -ReferencedAssemblies $references -ErrorAction Stop
$source=[CovenTokenIdentityControlled.CovenTokenSourceIdentity]::new(('a'*40),[CovenTokenIdentityControlled.CovenTokenSourceFileIdentity[]]@([CovenTokenIdentityControlled.CovenTokenSourceFileIdentity]::new('binding',('b'*64))),$true)
$hostProof=[CovenTokenIdentityControlled.CovenTokenHostIdentity]::new(('c'*64),[CovenTokenIdentityControlled.CovenTokenAssemblyIdentity[]]@([CovenTokenIdentityControlled.CovenTokenAssemblyIdentity]::new('corelib',('d'*64),[uint[]]@(10,0,0,0))),[CovenTokenIdentityControlled.CovenTokenLibraryIdentity[]]@([CovenTokenIdentityControlled.CovenTokenLibraryIdentity]::new('advapi32-system32',('e'*64)),[CovenTokenIdentityControlled.CovenTokenLibraryIdentity]::new('kernel32-system32',('f'*64))),$true)
$cases=0
foreach($identity in @($source,$hostProof)) {
    $bytes=[CovenTokenIdentityControlled.CovenTokenBindingPublic]::EncodeIdentity($identity)
    $value=[Text.UTF8Encoding]::new($false,$true).GetString($bytes) | ConvertFrom-Json -AsHashtable
    if(-not $value.Contains('beforeAfterMatch') -or -not $value.beforeAfterMatch -or $value.Contains('BeforeAfterMatch')){throw 'Token identity canonical casing failed'}
    $cases++
}
try {
    [CovenTokenIdentityControlled.CovenTokenBindingPublic]::EncodeIdentity([pscustomobject]@{Path='PRIVATE_TOKEN_PATH_SENTINEL'}) | Out-Null
    throw 'Unapproved token identity admitted'
} catch { if($_.Exception.Message -eq 'Unapproved token identity admitted'){throw} }
$cases++
try {
    [CovenTokenIdentityControlled.CovenTokenBindingPublic]::EncodeIdentity([CovenTokenIdentityControlled.UnapprovedIdentity]::new()) | Out-Null
    throw 'Serializable private identity admitted'
} catch {if($_.Exception.Message -eq 'Serializable private identity admitted'){throw}}
$cases++
$oversize=[CovenTokenIdentityControlled.CovenTokenSourceIdentity]::new(('a'*9000),[CovenTokenIdentityControlled.CovenTokenSourceFileIdentity[]]@(),$true)
try {
    [CovenTokenIdentityControlled.CovenTokenBindingPublic]::EncodeIdentity($oversize) | Out-Null
    throw 'Oversize token identity admitted'
} catch { if($_.Exception.Message -eq 'Oversize token identity admitted'){throw} }
$cases++
$entry=[CovenTokenIdentityControlled.CovenOwnedTokenCollector].GetMethod('Collect',[Reflection.BindingFlags]'Public,Static')
if($entry.GetParameters().Length -ne 0 -or $entry.ReturnType -ne [CovenTokenIdentityControlled.CovenTokenQueryResult]){throw 'Token collector entry accepts unexpected input'}
$cases++
$controlled=[CovenTokenIdentityControlled.TokenIdentityFixture]::Create()
$encoded=[CovenTokenIdentityControlled.CovenTokenBindingPublic]::EncodeProbe($controlled,$source,$hostProof)
$probe=[Text.UTF8Encoding]::new($false,$true).GetString($encoded.Payload) | ConvertFrom-Json -AsHashtable
if($encoded.Payload.Length -gt 32768 -or $encoded.ExitCode -ne 0 -or $probe.kind -cne 'coven.effective-token-probe.v1' -or $probe.component -isnot [string]){throw 'Token probe wrapper invalid'}
$component=$probe.component | ConvertFrom-Json -AsHashtable
if($component.kind -cne 'coven.effective-token-result.v1' -or $component.result.facts.context -ne 1){throw 'Token probe component casing changed'}
$cases++
$overProbe=[CovenTokenIdentityControlled.CovenTokenSourceIdentity]::new(('a'*33000),[CovenTokenIdentityControlled.CovenTokenSourceFileIdentity[]]@(),$true)
$escaped=$null
try {
    $escaped=[CovenTokenIdentityControlled.CovenTokenBindingPublic]::EncodeProbe($controlled,$overProbe,$hostProof)
    throw 'Oversize token probe admitted'
} catch {
    if($null -ne $escaped -or $_.Exception.InnerException.Message -cne 'Token probe output unavailable'){throw}
}
$cases++
# Never invoke the native entry here, including on Windows.
"token binding identity controlled PASS cases=$cases (no collector/native invocation)"
