# Builds the GameTable server for Linux and installs it on the live server (the port of the
# original's server/tools/build.ps1 + server/upload-server.bat).
#
#   pwsh tools/deploy-server.ps1            test, build, upload, back up, install, restart
#   pwsh tools/deploy-server.ps1 -DryRun    the same up to the upload, then run the new server on
#                                           the VM against a copy of the data (port 51299) and print
#                                           its log; the live service is untouched
#   -SkipTests                              skip `cargo test -p gametable-server`
#
# The Linux binary is built in WSL. The connection details live in tools/deploy-server.local.json
# (not committed):
#   { "host": "1.2.3.4", "user": "name", "keyPath": "C:\\Users\\me\\.ssh\\key.ppk" }
# PuTTY's plink and pscp must be on PATH. The remote half is tools/server/install.sh; the service
# and its management script are tools/server/gametable.service and tools/server/server.sh.
param([switch]$DryRun, [switch]$SkipTests)
$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot
$serverFiles = Join-Path $PSScriptRoot 'server'
$configPath = Join-Path $PSScriptRoot 'deploy-server.local.json'
if (-not (Test-Path $configPath)) {
  throw "Missing $configPath. Create it with: { ""host"": ""<ip>"", ""user"": ""<ssh user>"", ""keyPath"": ""<path to .ppk>"" }"
}
$config = Get-Content $configPath -Raw | ConvertFrom-Json
$target = "$($config.user)@$($config.host)"
$key = $config.keyPath
if (-not (Test-Path $key)) { throw "SSH key not found: $key" }

function Step([string]$message) { Write-Host "==> $message" -ForegroundColor Cyan }

# Runs a command on the server; returns its output lines, throws if it fails.
function Invoke-Remote([string]$command) {
  $output = & plink -batch -i $key $target $command 2>&1
  if ($LASTEXITCODE -ne 0) {
    $output | ForEach-Object { Write-Host $_ }
    throw "Remote command failed ($LASTEXITCODE): $command"
  }
  return $output
}

function Send-File([string]$localPath, [string]$remoteName) {
  & pscp -batch -q -i $key $localPath "${target}:$remoteName"
  if ($LASTEXITCODE -ne 0) { throw "Upload failed: $localPath" }
}

# Text files go up with Unix line endings, whatever the checkout uses.
function Send-TextFile([string]$localPath, [string]$remoteName, [hashtable]$replacements = @{}) {
  $text = [IO.File]::ReadAllText($localPath).Replace("`r`n", "`n")
  foreach ($name in $replacements.Keys) { $text = $text.Replace($name, $replacements[$name]) }
  $temporary = Join-Path $staging ([IO.Path]::GetFileName($remoteName))
  [IO.File]::WriteAllText($temporary, $text, (New-Object System.Text.UTF8Encoding($false)))
  Send-File $temporary $remoteName
}

function In-Wsl([string]$command) {
  $output = & wsl -e bash -lc $command
  if ($LASTEXITCODE -ne 0) { throw "WSL command failed: $command" }
  return $output
}

if (-not $SkipTests) {
  Step 'Testing the server'
  Push-Location $root
  try {
    cargo test -p gametable-server
    if ($LASTEXITCODE -ne 0) { throw 'Server tests failed' }
  } finally { Pop-Location }
}

Step 'Building the Linux server in WSL'
$wslRoot = (In-Wsl "wslpath -a '$($root -replace '\\', '/')'").Trim()
In-Wsl "cd '$wslRoot' && CARGO_TARGET_DIR=`$HOME/gametable-target cargo build --release -p gametable-server" | Out-Host
$binary = (In-Wsl 'echo $HOME/gametable-target/release/gametable-server').Trim()

Step 'Checking the C library version'
$needed = (In-Wsl "objdump -T '$binary' | grep -o 'GLIBC_[0-9.]*' | sed 's/GLIBC_//' | sort -uV | tail -1").Trim()
$installed = ((Invoke-Remote 'ldd --version | head -1') -split '\s+')[-1].Trim()
Write-Host "   the server needs glibc $needed; the VM has $installed"
if ([version]$needed -gt [version]$installed) {
  throw "The Linux build needs glibc $needed but the server has $installed. Build on an older Linux (e.g. the CI's Ubuntu 22.04)."
}

Step 'Packaging dist/server.zip'
$staging = Join-Path $root 'dist/server'
New-Item -ItemType Directory -Force $staging | Out-Null
$zip = Join-Path $root 'dist/server.zip'
$wslZip = (In-Wsl "wslpath -a '$($zip -replace '\\', '/')'").Trim()
# zipfile keeps the executable bit in the entry's external attributes (WSL has no `zip`).
In-Wsl "python3 -c `"import zipfile; info = zipfile.ZipInfo('gametable-server'); info.external_attr = 0o100755 << 16; info.compress_type = zipfile.ZIP_DEFLATED; zipfile.ZipFile('$wslZip', 'w').writestr(info, open('$binary', 'rb').read())`"" | Out-Null
Write-Host ("   {0:N1} MB" -f ((Get-Item $zip).Length / 1MB))

Step "Uploading to $($config.host)"
$remoteHome = (Invoke-Remote 'echo $HOME').Trim()
Send-File $zip 'server.zip'
Send-TextFile (Join-Path $serverFiles 'install.sh') 'gametable-install.sh'

if ($DryRun) {
  Step 'Dry run on the VM (port 51299, a copy of the data)'
  Invoke-Remote 'bash ~/gametable-install.sh dryrun; rm -f ~/gametable-install.sh' | ForEach-Object { Write-Host $_ }
  Step 'Dry run finished; the live service was not touched.'
  return
}

Send-TextFile (Join-Path $serverFiles 'server.sh') 'server.sh.new'
Send-TextFile (Join-Path $serverFiles 'gametable.service') 'gametable.service' @{ '{{USER}}' = $config.user; '{{HOME}}' = $remoteHome }

Step 'Installing (back up, switch service if needed, update, restart)'
& plink -batch -i $key $target 'bash ~/gametable-install.sh deploy; status=$?; rm -f ~/gametable-install.sh; exit $status' 2>&1 | ForEach-Object { Write-Host $_ }
$status = $LASTEXITCODE
Write-Host ''
Write-Host 'Rollback to the old Go server, if ever needed:'
Write-Host '  sudo systemctl disable --now gametable && sudo systemctl enable --now cardflux'
Write-Host '  (restore data/cardflux.db* from ~/backups/<stamp>/ first to undo data changes)'
if ($status -ne 0) { throw 'The gametable service is not running; see the log above.' }
Step 'Deployed.'
