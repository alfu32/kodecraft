# Native launcher. Argument boundaries, working directory and exit code survive.
$ErrorActionPreference = 'Stop'
$kodeNative = Join-Path $PSScriptRoot $(if ($env:OS -eq 'Windows_NT') { 'kode.exe' } else { 'kode' })
& $kodeNative @args
exit $LASTEXITCODE
