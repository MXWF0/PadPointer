[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Compiler,
    [Parameter(Mandatory)][string]$ReleaseDirectory
)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$version = (Select-String -LiteralPath (Join-Path $root 'Cargo.toml') -Pattern '^version = "(\d+\.\d+\.\d+)"$' | Select-Object -First 1).Matches[0].Groups[1].Value
$release = (Resolve-Path -LiteralPath $ReleaseDirectory).Path
$notice = Join-Path $release 'ThirdPartyLicenses.zip'
$setup = Join-Path $release "PadPointer-$version-Windows-x64-Setup.exe"
Compress-Archive -LiteralPath (Join-Path $release 'LICENSES') -DestinationPath $notice -Force
& $Compiler "/DVERSION=$version" "/DRELEASE_DIR=$release" "/DNOTICE_ZIP=$notice" "/DSETUP_EXE=$setup" (Join-Path $PSScriptRoot 'installer.nsi')
if ($LASTEXITCODE -ne 0) { throw "Installer compilation failed (exit $LASTEXITCODE)." }
Get-Item -LiteralPath $setup
