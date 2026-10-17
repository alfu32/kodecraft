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

# The MSIX contains the same portable payload and a full-trust console entry point.
$MsixStage = Join-Path $Root "target\msix-package-$Arch"
if (Test-Path $MsixStage) { Remove-Item $MsixStage -Recurse -Force }
Copy-Item $Payload $MsixStage -Recurse
$AssetsDir = Join-Path $MsixStage 'Assets'
New-Item $AssetsDir -ItemType Directory -Force | Out-Null
Add-Type -AssemblyName System.Drawing
foreach ($spec in @(@('StoreLogo',50),@('Square44x44Logo',44),@('Square150x150Logo',150))) {
  $bitmap = [System.Drawing.Bitmap]::new($spec[1],$spec[1])
  $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
  $graphics.Clear([System.Drawing.Color]::FromArgb(24,33,58))
  $font = [System.Drawing.Font]::new('Arial',[single]($spec[1] * 0.65),[System.Drawing.FontStyle]::Bold)
  $graphics.DrawString('K',$font,[System.Drawing.Brushes]::White,0,0)
  $bitmap.Save((Join-Path $AssetsDir "$($spec[0]).png"),[System.Drawing.Imaging.ImageFormat]::Png)
  $font.Dispose(); $graphics.Dispose(); $bitmap.Dispose()
}
$Manifest = @"
<?xml version="1.0" encoding="utf-8"?>
<Package xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10"
         xmlns:uap="http://schemas.microsoft.com/appx/manifest/uap/windows10"
         xmlns:rescap="http://schemas.microsoft.com/appx/manifest/foundation/windows10/restrictedcapabilities"
         IgnorableNamespaces="uap rescap">
  <Identity Name="Kode.KodeRs" Publisher="CN=Kode" Version="$Version.0" ProcessorArchitecture="$Arch" />
  <Properties><DisplayName>Kode</DisplayName><PublisherDisplayName>Kode</PublisherDisplayName>
    <Description>Kode terminal editor</Description><Logo>Assets\StoreLogo.png</Logo></Properties>
  <Resources><Resource Language="en-us" /></Resources>
  <Dependencies><TargetDeviceFamily Name="Windows.Desktop" MinVersion="10.0.17763.0" MaxVersionTested="10.0.22621.0" /></Dependencies>
  <Applications><Application Id="Kode" Executable="kode.exe" EntryPoint="Windows.FullTrustApplication">
    <uap:VisualElements DisplayName="Kode" Description="Kode terminal editor" BackgroundColor="transparent"
      Square150x150Logo="Assets\Square150x150Logo.png" Square44x44Logo="Assets\Square44x44Logo.png" />
  </Application></Applications>
  <Capabilities><rescap:Capability Name="runFullTrust" /></Capabilities>
</Package>
"@
Set-Content -Path (Join-Path $MsixStage 'AppxManifest.xml') -Value $Manifest -Encoding UTF8
$MakeAppx = Get-ChildItem 'C:\Program Files (x86)\Windows Kits\10\bin' -Recurse -Filter makeappx.exe |
  Where-Object { $_.FullName -match '\\x64\\' } | Sort-Object FullName -Descending | Select-Object -First 1
if (-not $MakeAppx) { throw 'Windows SDK makeappx.exe not found' }
$Msix = Join-Path $Assets "kode-$Version-windows-$Arch.msix"
& $MakeAppx.FullName pack /d $MsixStage /p $Msix /o
if ($LASTEXITCODE -ne 0 -or -not (Test-Path $Msix)) { throw 'MSIX build failed' }
foreach ($artifact in @($Msi,$Msix)) {
  $digest = (Get-FileHash $artifact -Algorithm SHA256).Hash.ToLowerInvariant()
  "$digest  $(Split-Path $artifact -Leaf)" | Set-Content "$artifact.sha256"
}
