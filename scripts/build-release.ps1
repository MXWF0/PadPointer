[CmdletBinding()]
param([switch]$SkipBuild, [string]$OutputDirectory = 'release')
$ErrorActionPreference = 'Stop'
Set-Location -LiteralPath (Split-Path -Parent $PSScriptRoot)
$target = 'x86_64-pc-windows-msvc'
if (-not $SkipBuild) {
    $oldFlags = $env:CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS
    $env:CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS = "$oldFlags -C target-feature=+crt-static".Trim()
    try {
        cargo build --locked --release --target $target
    } finally {
        $env:CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS = $oldFlags
    }
    if ($LASTEXITCODE -ne 0) { throw 'Release build failed.' }
}
$exe = "target/$target/release/padpointer.exe"
if (-not (Test-Path -LiteralPath $exe)) { throw "Missing $exe" }

$stage = Join-Path (Get-Location).Path "target/package-$([guid]::NewGuid())"
$licenses = Join-Path $stage 'LICENSES'
New-Item -ItemType Directory -Path $stage,$licenses,(Join-Path $stage 'docs') -Force | Out-Null
Copy-Item -LiteralPath $exe -Destination (Join-Path $stage 'PadPointer.exe')
Copy-Item -LiteralPath README.md,LICENSE -Destination $stage
Copy-Item -LiteralPath docs/windows-test.md -Destination (Join-Path $stage 'docs')
Set-Content -Encoding ascii (Join-Path $stage 'portable.flag') 'PadPointer portable mode'

$meta = cargo metadata --offline --locked --format-version 1 --filter-platform $target
if ($LASTEXITCODE -ne 0) { throw 'Unable to read dependency metadata.' }
$meta = $meta | ConvertFrom-Json
$graph = cargo tree --offline --locked --target $target --edges normal --prefix none --format '{p}'
if ($LASTEXITCODE -ne 0) { throw 'Unable to read enabled dependencies.' }
$enabled = [System.Collections.Generic.HashSet[string]]::new()
foreach ($line in $graph) {
    if ($line -match '^([\w-]+) v(\S+)') { [void]$enabled.Add("$($Matches[1])-$($Matches[2])") }
}
$summary = foreach ($package in $meta.packages) {
    $label = "$($package.name)-$($package.version)"
    if ($package.id -eq $meta.resolve.root -or -not $enabled.Contains($label)) { continue }
    $root = Split-Path -Parent $package.manifest_path
    $files = @(Get-ChildItem -LiteralPath $root -File | Where-Object Name -Match '^(LICENSE|LICENCE|COPYING|NOTICE|COPYRIGHT|UNLICENSE|OFL|THIRDPARTY|THIRD.PARTY)($|[._-])')
    $licenseDir = Join-Path $root 'LICENSES'
    if (Test-Path -LiteralPath $licenseDir -PathType Container) { $files += Get-ChildItem -LiteralPath $licenseDir -File }
    if ($package.license_file) { $files += Get-Item -LiteralPath (Join-Path $root $package.license_file) }
    foreach ($file in ($files | Sort-Object FullName -Unique)) {
        $dest = Join-Path $licenses "$label-$($file.Name)"
        Copy-Item -LiteralPath $file.FullName -Destination $dest
        if ($file.LastWriteTime.Year -lt 1980) { (Get-Item -LiteralPath $dest).LastWriteTime = [datetime]'1980-01-01' }
    }
    "$label | $($package.license) | $($package.repository)"
}
$summary | Sort-Object | Set-Content -Encoding utf8 (Join-Path $licenses 'Third-party-components.txt')
$out = Join-Path (Get-Location).Path $OutputDirectory
New-Item -ItemType Directory -Path $out -Force | Out-Null
Copy-Item -Path (Join-Path $stage '*') -Destination $out -Recurse -Force
Compress-Archive -Path (Join-Path $stage '*') -DestinationPath (Join-Path $out 'PadPointer-Windows-x64-Portable.zip') -Force
Get-Item (Join-Path $out 'PadPointer.exe'),(Join-Path $out 'PadPointer-Windows-x64-Portable.zip') | Select-Object FullName,Length


