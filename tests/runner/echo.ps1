$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
[Console]::WriteLine((ConvertTo-Json -Compress -InputObject @{args=@($args);cwd=(Get-Location).Path;env=$env:RSCRIPTO_TEST}))
[Console]::Error.WriteLine('stderr-visible')
exit 0
