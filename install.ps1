# Aspen installer (Windows).
#
#   irm https://raw.githubusercontent.com/methodify/aspen/main/install.ps1 | iex
#
# Installs the latest release binary to %USERPROFILE%\.local\bin\aspen.exe
# (the same construct as Linux/macOS — next to claude).
# While the repo is private, set GITHUB_TOKEN (a PAT with repo read access).
#
# Environment:
#   ASPEN_VERSION       tag to install (default: latest)
#   ASPEN_INSTALL_DIR   target dir (default: %USERPROFILE%\.local\bin)
#   ASPEN_RELEASE_REPO  owner/repo (default: methodify/aspen)
#   GITHUB_TOKEN        auth token (required while the repo is private)

$ErrorActionPreference = "Stop"

$repo = if ($env:ASPEN_RELEASE_REPO) { $env:ASPEN_RELEASE_REPO } else { "methodify/aspen" }
$api = if ($env:ASPEN_GITHUB_API) { $env:ASPEN_GITHUB_API } else { "https://api.github.com" }
$installDir = if ($env:ASPEN_INSTALL_DIR) { $env:ASPEN_INSTALL_DIR } else { Join-Path $env:USERPROFILE ".local\bin" }

$arch = if ([Environment]::Is64BitOperatingSystem -and
            [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture -eq "Arm64") {
  "aarch64"
} else {
  "x86_64"
}
$target = "$arch-pc-windows-msvc"
$asset = "aspen-$target.exe"

$headers = @{ "Accept" = "application/vnd.github+json"; "User-Agent" = "aspen-install" }
if ($env:GITHUB_TOKEN) { $headers["Authorization"] = "Bearer $($env:GITHUB_TOKEN)" }

$releaseUrl = if ($env:ASPEN_VERSION) {
  $tag = $env:ASPEN_VERSION
  if ($tag -match '^\d') { $tag = "v$tag" }
  "$api/repos/$repo/releases/tags/$tag"
} else {
  "$api/repos/$repo/releases/latest"
}

try {
  $release = Invoke-RestMethod -Uri $releaseUrl -Headers $headers
} catch {
  throw "could not fetch release info from $repo (private repo? set GITHUB_TOKEN): $_"
}
$tag = $release.tag_name

$binAsset = $release.assets | Where-Object { $_.name -eq $asset }
$sumsAsset = $release.assets | Where-Object { $_.name -eq "SHA256SUMS" }
if (-not $binAsset) { throw "release $tag has no asset $asset" }
if (-not $sumsAsset) { throw "release $tag has no SHA256SUMS" }

# Asset API download: works for private repos (browser_download_url does not).
$dlHeaders = $headers.Clone()
$dlHeaders["Accept"] = "application/octet-stream"

$tmp = Join-Path ([System.IO.Path]::GetTempPath()) "aspen-install-$PID"
New-Item -ItemType Directory -Force -Path $tmp | Out-Null
try {
  Write-Host "downloading aspen $tag ($target) ..."
  $binPath = Join-Path $tmp $asset
  $sumsPath = Join-Path $tmp "SHA256SUMS"
  Invoke-WebRequest -Uri $binAsset.url -Headers $dlHeaders -OutFile $binPath
  Invoke-WebRequest -Uri $sumsAsset.url -Headers $dlHeaders -OutFile $sumsPath

  $expected = (Get-Content $sumsPath | Where-Object { $_ -match "  $([regex]::Escape($asset))`$" }) -split '\s+' | Select-Object -First 1
  if (-not $expected) { throw "SHA256SUMS has no entry for $asset" }
  $actual = (Get-FileHash -Algorithm SHA256 $binPath).Hash.ToLower()
  if ($actual -ne $expected.ToLower()) { throw "checksum mismatch: expected $expected, got $actual" }

  New-Item -ItemType Directory -Force -Path $installDir | Out-Null
  $dest = Join-Path $installDir "aspen.exe"
  # A running aspen.exe can't be overwritten, but it can be renamed aside;
  # the daemon deletes the .old on its next start.
  if (Test-Path $dest) {
    $old = "$dest.old"
    Remove-Item -Force $old -ErrorAction SilentlyContinue
    try { Move-Item -Force $dest $old } catch {}
  }
  Move-Item -Force $binPath $dest
  Write-Host "installed: $dest ($tag)"

  $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
  if (($userPath -split ';') -notcontains $installDir) {
    Write-Host ""
    Write-Host "note: $installDir is not on your PATH. Add it (new shells) with:"
    Write-Host "  [Environment]::SetEnvironmentVariable('Path', `"$installDir;`" + [Environment]::GetEnvironmentVariable('Path','User'), 'User')"
  }

  Write-Host ""
  Write-Host "get started:  aspen up -d   ->  http://127.0.0.1:7420"
} finally {
  Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}
