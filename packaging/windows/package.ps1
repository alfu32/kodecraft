param(
  [Parameter(Mandatory=$true)][ValidateSet('x64','arm64')][string]$Arch,
  [Parameter(Mandatory=$true)][string]$Version
)
$ErrorActionPreference = 'Stop'
$Target = if ($Arch -eq 'arm64') { 'aarch64-pc-windows-msvc' } else { 'x86_64-pc-windows-msvc' }
$Root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$Assets = Join-Path $Root 'target\release-assets'
$Package = "kode-$Version-$Target"
$Archive = Join-Path $Assets "$Package.zip"
$Stage = Join-Path $Root "target\windows-package-$Arch"
if (Test-Path $Stage) { Remove-Item $Stage -Recurse -Force }
Expand-Archive $Archive -DestinationPath $Stage
$Payload = Join-Path $Stage $Package
if (-not (Test-Path (Join-Path $Payload 'jdbc\kode-jdbc-bridge.jar'))) { throw 'JDBC adapter missing from payload' }

dotnet tool install --global wix --version 5.0.2
if ($LASTEXITCODE -ne 0) { throw 'WiX installation failed' }
$env:PATH += ";$env:USERPROFILE\.dotnet\tools"
$Msi = Join-Path $Assets "kode-$Version-windows-$Arch.msi"
wix build (Join-Path $PSScriptRoot 'kode.wxs') -arch $Arch -d "Version=$Version" -d "Stage=$Payload" -o $Msi
if ($LASTEXITCODE -ne 0 -or -not (Test-Path $Msi)) { throw 'MSI build failed' }
# WiX emits debug symbol files beside the installer; they are build outputs, not release assets.
Get-ChildItem -Path $Assets -Filter '*.wixpdb' -File | Remove-Item -Force

# The MSIX contains the same portable payload with the Store identity from msstore-info.txt.
. (Join-Path $PSScriptRoot 'store-identity.ps1')
$Identity = Get-StoreIdentity (Join-Path $Root 'packaging\msstore-info.txt')
$MsixStage = Join-Path $Root "target\msix-package-$Arch"
if (Test-Path $MsixStage) { Remove-Item $MsixStage -Recurse -Force }
Copy-Item $Payload $MsixStage -Recurse
Copy-Item (Join-Path $PSScriptRoot 'Assets') (Join-Path $MsixStage 'Assets') -Recurse
$Manifest = Join-Path $MsixStage 'AppxManifest.xml'
Set-Content -Path $Manifest -Encoding UTF8 -Value (New-StoreManifest (Join-Path $PSScriptRoot 'AppxManifest.xml') $Identity $Version $Arch)
# Windows only finds the taskbar and Start icon sizes through a resource index.
$MakePri = Find-WindowsSdkTool 'makepri.exe'
$PriConfig = Join-Path $Root "target\msix-priconfig-$Arch.xml"
& $MakePri createconfig /cf $PriConfig /dq en-US /pv 10.0.0 /o
if ($LASTEXITCODE -ne 0) { throw 'makepri createconfig failed' }
& $MakePri new /pr $MsixStage /cf $PriConfig /mn $Manifest /of (Join-Path $MsixStage 'resources.pri') /o
if ($LASTEXITCODE -ne 0) { throw 'makepri new failed' }
$Msix = Join-Path $Assets "kode-$Version-windows-$Arch.msix"
& (Find-WindowsSdkTool 'makeappx.exe') pack /d $MsixStage /p $Msix /o
if ($LASTEXITCODE -ne 0 -or -not (Test-Path $Msix)) { throw 'MSIX build failed' }
foreach ($artifact in @($Msi,$Msix)) {
  $digest = (Get-FileHash $artifact -Algorithm SHA256).Hash.ToLowerInvariant()
  "$digest  $(Split-Path $artifact -Leaf)" | Set-Content "$artifact.sha256"
}
