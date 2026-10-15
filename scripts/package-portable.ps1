param([switch]$SkipBuild, [string]$OutputDirectory = 'release')
$ErrorActionPreference = 'Stop'
$taskRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
Push-Location $taskRoot
try {
    if (-not $SkipBuild) {
        & npm.cmd ci
        if ($LASTEXITCODE -ne 0) { throw 'npm ci failed' }
        & npm.cmd run tauri -- build --no-bundle
        if ($LASTEXITCODE -ne 0) { throw 'Tauri build failed' }
    }
    $manifest = Get-Content -LiteralPath (Join-Path $taskRoot 'package.json') -Raw | ConvertFrom-Json
    if ($manifest.version -notmatch '^\d+\.\d+\.\d+(-[A-Za-z0-9.-]+)?$') { throw 'Invalid release version' }
    $releaseName = "RScriptoBox-$($manifest.version)-windows-x64"
    $releaseRoot = [System.IO.Path]::GetFullPath($OutputDirectory, $taskRoot)
    $packageRoot = Join-Path $releaseRoot $releaseName
    if (Test-Path -LiteralPath (Join-Path $packageRoot 'data')) { throw 'Packaging folder contains user data; use a clean release directory to avoid including it' }
    $null = New-Item -ItemType Directory -Force -Path $packageRoot
    $executable = Join-Path $taskRoot 'src-tauri\target\release\rscripto-box.exe'
    if (-not (Test-Path -LiteralPath $executable)) { throw 'Release executable is missing' }
    Copy-Item -LiteralPath $executable -Destination (Join-Path $packageRoot 'RScriptoBox.exe')
    [System.IO.File]::WriteAllText((Join-Path $packageRoot 'portable.flag'), '')
    Copy-Item -LiteralPath (Join-Path $taskRoot 'README.md') -Destination (Join-Path $packageRoot 'README.md')
    $taskEntries = [ordered]@{
        'RScriptoBox.exe' = (Join-Path $packageRoot 'RScriptoBox.exe')
        'portable.flag' = (Join-Path $packageRoot 'portable.flag')
        'README.md' = (Join-Path $packageRoot 'README.md')
    }
    foreach ($taskDocument in @('README_EN.md','LICENSE','THIRD-PARTY-NOTICES.txt','CONTRIBUTING.md','CODE_OF_CONDUCT.md','SECURITY.md','SUPPORT.md','GITHUB_PREPARATION.md')) {
        $taskDestination = Join-Path $packageRoot $taskDocument
        Copy-Item -LiteralPath (Join-Path $taskRoot $taskDocument) -Destination $taskDestination
        $taskEntries.Add($taskDocument, $taskDestination)
    }
    foreach ($taskFolder in @('examples','docs')) {
        foreach ($taskFile in Get-ChildItem -LiteralPath (Join-Path $taskRoot $taskFolder) -Recurse -File) {
            $taskRelative = [System.IO.Path]::GetRelativePath($taskRoot, $taskFile.FullName)
            $taskDestination = Join-Path $packageRoot $taskRelative
            $null = New-Item -ItemType Directory -Force -Path (Split-Path -Parent $taskDestination)
            Copy-Item -LiteralPath $taskFile.FullName -Destination $taskDestination -Force
            $taskEntries.Add($taskRelative.Replace('\','/'), $taskDestination)
        }
    }
    $archive = Join-Path $releaseRoot "$releaseName-portable.zip"
    $temporaryArchive = "$archive.tmp"
    $taskZipStream = [System.IO.File]::Create($temporaryArchive)
    $taskZip = [System.IO.Compression.ZipArchive]::new($taskZipStream, [System.IO.Compression.ZipArchiveMode]::Create)
    try {
        foreach ($taskEntry in $taskEntries.GetEnumerator()) {
            $null = [System.IO.Compression.ZipFileExtensions]::CreateEntryFromFile(
                $taskZip, $taskEntry.Value, $taskEntry.Key, [System.IO.Compression.CompressionLevel]::Optimal)
        }
    } finally { $taskZip.Dispose(); $taskZipStream.Dispose() }
    Move-Item -LiteralPath $temporaryArchive -Destination $archive -Force
    $hash = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant()
    [System.IO.File]::WriteAllText("$archive.sha256", "$hash  $([System.IO.Path]::GetFileName($archive))" + [Environment]::NewLine)
    Get-Item -LiteralPath $archive, (Join-Path $packageRoot 'RScriptoBox.exe') | Select-Object FullName, Length
} finally { Pop-Location }
