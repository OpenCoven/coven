function Write-CovenPreflightReceipt {
    param([Parameter(Mandatory)]$Result)
    # New owned run root; public and raw output are siblings, never nested.
    $root=[IO.Directory]::CreateTempSubdirectory('coven-native-preflight-').FullName
    $public=Join-Path $root 'public'
    $private=Join-Path $root 'private'
    [IO.Directory]::CreateDirectory($public) | Out-Null
    [IO.Directory]::CreateDirectory($private) | Out-Null
    $utf8=[Text.UTF8Encoding]::new($false)
    [IO.File]::WriteAllText((Join-Path $private 'queries.json'),($Result.rawResults | ConvertTo-Json -Depth 20),$utf8)
    [IO.File]::WriteAllText((Join-Path $public 'receipt.json'),($Result.public | ConvertTo-Json -Depth 20),$utf8)
    # Internal caller result. Workflow may upload ONLY publicDirectory/receipt.json.
    [pscustomobject]@{root=$root;publicDirectory=$public;privateDirectory=$private}
}
