function Assert-CovenSourceStable {
    param([Parameter(Mandatory)][object[]]$Before,[Parameter(Mandatory)][string]$Directory)
    foreach ($source in $Before) {
        $after=(Get-FileHash -LiteralPath (Join-Path $Directory $source.file) -Algorithm SHA256 -ErrorAction Stop).Hash.ToLowerInvariant()
        if ($after -cne $source.sha256) { throw 'Preflight source changed' }
    }
}
