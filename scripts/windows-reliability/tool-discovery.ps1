function Get-CovenInstalledToolIdentities {
    param(
        [scriptblock]$PathLookup = { param($name) $c=Get-Command $name -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1; if ($c) { $c.Source } },
        [string]$WptDirectory = $(if (${env:ProgramFiles(x86)}) { Join-Path ${env:ProgramFiles(x86)} 'Windows Kits/10/Windows Performance Toolkit' } else { '' }),
        [string]$SystemDirectory = $(if ($env:SystemRoot) { Join-Path $env:SystemRoot 'System32' } else { '' })
    )
    $identities=@{}
    foreach ($tool in @('xperf','tracelog','tracerpt','WPAExporter','wevtutil','whoami')) {
        $path=$null; $kind='path'
        if ($tool -in @('wevtutil','whoami')) {
            $kind='system32'
            if ($SystemDirectory) { $path=Join-Path $SystemDirectory "$tool.exe" }
        } else {
            $path=& $PathLookup "$tool.exe"
            if (-not $path -and $WptDirectory) {
                $kind='wpt'
                $path=Join-Path $WptDirectory "$tool.exe"
            }
        }
        if ($path) {
            # Inspect the selected candidate once; do not silently replace a broken identity.
            $identities[$tool]=Get-CovenToolIdentity -Tool $tool -AbsolutePath $path -LocationKind $kind
        } else {
            $identities[$tool]=[pscustomobject]@{path=$null;summary=[pscustomobject]@{tool=$tool;locationKind=$kind;present=$false;identityFailed=$false;sha256=$null;fileVersion=$null;signatureStatus='unavailable';microsoftSigner=$false;originalFilenameMatched=$false;originalFilenameClass='not-observed';stableHashMatched=$null;admitted=$false}}
        }
    }
    $identities
}
