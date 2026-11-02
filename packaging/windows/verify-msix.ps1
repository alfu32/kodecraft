# Installs a throwaway-signed copy of the built MSIX and checks what Store customers will get:
# the Store identity, and that every command alias starts the app. The release asset stays
# unsigned because the Store signs packages itself.
param(
  [Parameter(Mandatory=$true)][ValidateSet('x64','arm64')][string]$Arch,
  [Parameter(Mandatory=$true)][string]$Version
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'store-identity.ps1')
$Root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$Identity = Get-StoreIdentity (Join-Path $Root 'packaging\msstore-info.txt')
$Publisher = $Identity['Package/Identity/Publisher']
$Name = $Identity['Package/Identity/Name']
$Msix = Join-Path $Root "target\release-assets\kode-$Version-windows-$Arch.msix"
$Work = Join-Path $Root "target\msix-verify-$Arch"
if (Test-Path $Work) { Remove-Item $Work -Recurse -Force }
New-Item $Work -ItemType Directory | Out-Null

$Signed = Join-Path $Work 'signed.msix'
Copy-Item $Msix $Signed
$Certificate = New-SelfSignedCertificate -Type Custom -Subject $Publisher -KeyUsage DigitalSignature `
  -FriendlyName 'kodecraft MSIX check' -CertStoreLocation 'Cert:\CurrentUser\My' `
  -TextExtension @('2.5.29.37={text}1.3.6.1.5.5.7.3.3', '2.5.29.19={text}')
$Password = ConvertTo-SecureString -String (New-Guid).Guid -AsPlainText -Force
$Pfx = Join-Path $Work 'check.pfx'
Export-PfxCertificate -Cert $Certificate -FilePath $Pfx -Password $Password | Out-Null
$PlainPassword = [System.Net.NetworkCredential]::new('', $Password).Password
& (Find-WindowsSdkTool 'signtool.exe') sign /fd SHA256 /f $Pfx /p $PlainPassword $Signed
if ($LASTEXITCODE -ne 0) { throw 'Signing the check copy failed' }
Import-PfxCertificate -FilePath $Pfx -CertStoreLocation 'Cert:\LocalMachine\TrustedPeople' -Password $Password | Out-Null

Add-AppxPackage -Path $Signed
try {
  $Installed = Get-AppxPackage -Name $Name
  if (-not $Installed) { throw "$Name is not installed" }
  if ($Installed.PackageFamilyName -cne $Identity['Package Family Name (PFN)']) {
    throw "Package family $($Installed.PackageFamilyName) differs from the Store's $($Identity['Package Family Name (PFN)'])"
  }
  $Expected = ConvertTo-StoreVersion $Version
  if ([string]$Installed.Version -ne $Expected) { throw "Installed version $($Installed.Version), expected $Expected" }
  foreach ($Alias in 'koders', 'kode', 'kd') {
    $Output = & (Join-Path $env:LOCALAPPDATA "Microsoft\WindowsApps\$Alias.exe") --version 2>&1 | Out-String
    if ($LASTEXITCODE -ne 0 -or $Output -notmatch [regex]::Escape($Version)) {
      throw "Alias $Alias did not report version $Version (exit $LASTEXITCODE): $Output"
    }
    Write-Output "$Alias -> $($Output.Trim())"
  }
} finally {
  Get-AppxPackage -Name $Name | Remove-AppxPackage
  Remove-Item "Cert:\LocalMachine\TrustedPeople\$($Certificate.Thumbprint)" -ErrorAction SilentlyContinue
  Remove-Item "Cert:\CurrentUser\My\$($Certificate.Thumbprint)" -ErrorAction SilentlyContinue
}
