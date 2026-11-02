# Shared by package.ps1 and verify-msix.ps1.

# msstore-info.txt is Partner Center's "Product identity" page pasted as label/value line pairs.
function Get-StoreIdentity([string]$Path) {
  $lines = @(Get-Content $Path | ForEach-Object { $_.Trim() } | Where-Object { $_ })
  $values = @{}
  for ($i = 0; $i + 1 -lt $lines.Count; $i += 2) { $values[$lines[$i]] = $lines[$i + 1] }
  foreach ($key in 'Package/Identity/Name', 'Package/Identity/Publisher', 'Package/Properties/PublisherDisplayName', 'Package Family Name (PFN)') {
    if (-not $values[$key]) { throw "$key is missing from $Path" }
  }
  $values
}

# The Store rejects a package version whose first number is 0 and reserves the fourth.
# Adding 1 to the major version keeps every release ascending, including after a real 1.0.
function ConvertTo-StoreVersion([string]$Version) {
  if ($Version -notmatch '^(\d+)\.(\d+)\.(\d+)$') { throw "The Store needs a plain major.minor.patch version, not $Version" }
  "$([int]$Matches[1] + 1).$($Matches[2]).$($Matches[3]).0"
}

function New-StoreManifest([string]$Template, [hashtable]$Identity, [string]$Version, [string]$Arch) {
  (Get-Content $Template -Raw).
    Replace('@IDENTITY_NAME@', $Identity['Package/Identity/Name']).
    Replace('@PUBLISHER@', $Identity['Package/Identity/Publisher']).
    Replace('@PUBLISHER_DISPLAY_NAME@', $Identity['Package/Properties/PublisherDisplayName']).
    Replace('@VERSION@', (ConvertTo-StoreVersion $Version)).
    Replace('@ARCH@', $Arch)
}

function Find-WindowsSdkTool([string]$Name) {
  $tool = Get-ChildItem 'C:\Program Files (x86)\Windows Kits\10\bin' -Recurse -Filter $Name |
    Where-Object { $_.FullName -match '\\x64\\' } | Sort-Object FullName -Descending | Select-Object -First 1
  if (-not $tool) { throw "Windows SDK $Name not found" }
  $tool.FullName
}
