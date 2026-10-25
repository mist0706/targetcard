<#
.SYNOPSIS
    Build the targetcard MSI on Windows: download the latest source from GitHub,
    install the toolchain if needed, run the offline Tauri build.

.DESCRIPTION
    Run from an Administrator PowerShell. Everything it installs is a normal
    developer tool: Rust (rustup), Visual Studio Build Tools with the C++
    workload, Node LTS, and the WebView2 runtime. Nothing touches the game.

    No zip to keep track of. By default the script asks GitHub for the latest
    commit on the fork, downloads exactly that commit's archive, and prints the
    commit, the version and the archive hash BEFORE it starts compiling, so a
    stale or wrong source can never be mistaken for a successful build.

    Safe to re-run: each install step checks first, and -BuildOnly skips the
    installs once the toolchain is present. Your build output in
    src-tauri\target survives a source refresh.

    Disk: plan for about 8 GB free (VS Build Tools ~4 GB, cargo target ~2 GB,
    node_modules ~0.5 GB).

.PARAMETER Repo
    GitHub repository to build from. Default: mist0706/targetcard.

.PARAMETER Ref
    Branch, tag or commit to build. Default: main.

.PARAMETER ZipPath
    Build from a local source archive instead of downloading. Escape hatch for
    offline machines and for testing an unmerged change.

.PARAMETER WorkDir
    Where the source is unpacked and built. Keep it short; Tauri paths get long.

.PARAMETER SetupOnly
    Install the toolchain and stop before building.

.PARAMETER BuildOnly
    Build without checking or installing anything.

.PARAMETER NoInstall
    Build but do not install. Without this the MSI is installed (or upgraded) at
    the end, which is the normal path: a newer version replaces the old one.

.PARAMETER CollectorToken
    Write %APPDATA%\com.targetcard.desktop\diagnostics.json so the app uploads
    its debug log to $CollectorUrl every five minutes. Saved once and reused by
    later runs; the token stays out of the repository.

.PARAMETER CollectorUrl
    Where those logs go. Defaults to the collector on the LAN.

.PARAMETER Uninstall
    Remove an installed targetcard and stop.

.PARAMETER Force
    Re-unpack the source even if WorkDir already exists (your build output in
    src-tauri\target is preserved).
#>
[CmdletBinding()]
param(
    [string]$Repo = "mist0706/targetcard",
    [string]$Ref = "main",
    [string]$ZipPath,
    [string]$WorkDir = "C:\targetcard",
    [switch]$SetupOnly,
    [switch]$BuildOnly,
    [switch]$Force,
    [switch]$NoInstall,
    [switch]$Uninstall,
    [string]$CollectorUrl = "http://10.7.1.235:8791",
    [string]$CollectorToken
)

# Bumped whenever this script changes, and printed, so a stale copy in Downloads
# or %TEMP% says so instead of failing on a parameter it has never heard of.
$ScriptStamp = "2026-10-25T12:40Z"

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"      # speeds up Invoke-WebRequest a lot
# Windows PowerShell 5.1 still negotiates TLS 1.0 by default and GitHub refuses it.
try { [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12 } catch { }
Add-Type -AssemblyName System.IO.Compression.FileSystem

function Write-Step([string]$Text) { Write-Host "`n== $Text ==" -ForegroundColor Cyan }
function Write-Ok([string]$Text)   { Write-Host "   $Text" -ForegroundColor Green }
function Write-Warn2([string]$Text){ Write-Host "   $Text" -ForegroundColor Yellow }

function Assert-Admin {
    $id = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = New-Object Security.Principal.WindowsPrincipal($id)
    if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw "Run this from an Administrator PowerShell (right-click > Run as administrator)."
    }
}

function Update-SessionPath {
    # Pick up tools installed during this session without restarting the shell.
    $machine = [Environment]::GetEnvironmentVariable("Path", "Machine")
    $user = [Environment]::GetEnvironmentVariable("Path", "User")
    $env:Path = "$machine;$user"
    $cargo = Join-Path $env:USERPROFILE ".cargo\bin"
    if ((Test-Path $cargo) -and ($env:Path -notlike "*$cargo*")) { $env:Path = "$cargo;$env:Path" }
}

function Test-Cargo {
    Update-SessionPath
    $exe = Join-Path $env:USERPROFILE ".cargo\bin\cargo.exe"
    if (Test-Path $exe) { return $true }
    return [bool](Get-Command cargo -ErrorAction SilentlyContinue)
}

function Test-VcTools {
    $vswhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"
    if (-not (Test-Path $vswhere)) { return $false }
    $path = & $vswhere -products * -latest `
        -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
        -property installationPath 2>$null
    return [bool]$path
}

function Test-Node {
    Update-SessionPath
    return [bool](Get-Command node -ErrorAction SilentlyContinue) -and
           [bool](Get-Command npm -ErrorAction SilentlyContinue)
}

function Test-WebView2 {
    foreach ($root in ${env:ProgramFiles(x86)}, $env:ProgramFiles) {
        if ($root -and (Test-Path (Join-Path $root "Microsoft\EdgeWebView\Application"))) { return $true }
    }
    return $false
}

function Install-WithWinget([string]$Id, [string]$Override) {
    if (-not (Get-Command winget -ErrorAction SilentlyContinue)) { return $false }
    Write-Host "   winget install $Id (this can take a while)"
    $wgArgs = @("install", "--id", $Id, "--exact", "--silent",
                "--accept-source-agreements", "--accept-package-agreements")
    if ($Override) { $wgArgs += @("--override", $Override) }
    & winget @wgArgs
    Update-SessionPath
    return $LASTEXITCODE -eq 0
}

function Install-Rust {
    Write-Step "Rust toolchain"
    if (Test-Cargo) { Write-Ok "cargo already present"; return }
    $rustup = Join-Path $env:TEMP "rustup-init.exe"
    Write-Host "   downloading rustup-init.exe"
    Invoke-WebRequest "https://win.rustup.rs/x86_64" -OutFile $rustup -UseBasicParsing
    & $rustup -y --profile minimal --default-toolchain stable
    if ($LASTEXITCODE -ne 0) { throw "rustup install failed with exit code $LASTEXITCODE" }
    Update-SessionPath
    Write-Ok ("cargo " + (& (Join-Path $env:USERPROFILE ".cargo\bin\cargo.exe") --version))
}

function Install-VcTools {
    Write-Step "Visual Studio Build Tools (C++ workload, Windows SDK)"
    if (Test-VcTools) { Write-Ok "MSVC C++ toolchain already present"; return }
    $override = "--quiet --wait --norestart --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
    if (-not (Install-WithWinget "Microsoft.VisualStudio.2022.BuildTools" $override)) {
        Write-Warn2 "winget unavailable or failed; using the official bootstrapper"
        $exe = Join-Path $env:TEMP "vs_BuildTools.exe"
        Invoke-WebRequest "https://aka.ms/vs/17/release/vs_BuildTools.exe" -OutFile $exe -UseBasicParsing
        Start-Process -FilePath $exe -ArgumentList $override -Wait
    }
    if (-not (Test-VcTools)) {
        throw "MSVC C++ tools still not detected. Open the Visual Studio Installer and add 'Desktop development with C++'."
    }
    Write-Ok "MSVC toolchain detected"
}

function Install-Node {
    Write-Step "Node.js LTS"
    if (Test-Node) { Write-Ok ("node " + (& node --version)); return }
    if (-not (Install-WithWinget "OpenJS.NodeJS.LTS" $null)) {
        throw "Install Node.js LTS from https://nodejs.org/en/download and re-run this script."
    }
    if (-not (Test-Node)) { throw "Node still not on PATH. Close this window, open a new Administrator PowerShell, and re-run." }
    Write-Ok ("node " + (& node --version))
}

function Install-WebView2 {
    Write-Step "WebView2 runtime"
    if (Test-WebView2) { Write-Ok "already present"; return }
    if (-not (Install-WithWinget "Microsoft.EdgeWebView2Runtime" $null)) {
        Write-Warn2 "Falling back to the Evergreen bootstrapper"
        $exe = Join-Path $env:TEMP "MicrosoftEdgeWebview2Setup.exe"
        Invoke-WebRequest "https://go.microsoft.com/fwlink/p/?LinkId=2124703" -OutFile $exe -UseBasicParsing
        Start-Process -FilePath $exe -ArgumentList "/silent /install" -Wait
    }
    if (Test-WebView2) { Write-Ok "installed" } else { Write-Warn2 "not detected; the app may still run if Edge is installed" }
}

function Get-SourceArchive {
    # Download exactly one commit from GitHub. The commit SHA, not a branch name
    # and not a file date, is the identity of what gets built.
    Write-Step "Source download"
    Write-Ok ("repo: $Repo   ref: $Ref")

    # Resolve the ref to a commit SHA. api.github.com is CDN-cached and can serve
    # a commit that is a minute stale, which is exactly the confusion this script
    # exists to end: ask git first (authoritative), and cache-bust the API.
    $sha = $null; $when = ""; $subject = ""
    if ((Get-Command git -ErrorAction SilentlyContinue) -and ($Ref -notmatch '^[0-9a-f]{7,40}$')) {
        try {
            $line = & git ls-remote "https://github.com/$Repo" $Ref 2>$null | Select-Object -First 1
            if ($line) { $sha = ($line -split '\s+')[0] }
        } catch { }
    }
    $apiUrl = if ($sha) { "https://api.github.com/repos/$Repo/commits/$sha" }
              else      { "https://api.github.com/repos/$Repo/commits/" + $Ref + "?cb=" + [guid]::NewGuid().ToString("N") }
    try {
        $commit = Invoke-RestMethod -UseBasicParsing -TimeoutSec 30 -Uri $apiUrl `
            -Headers @{ "User-Agent" = "targetcard-build"; "Accept" = "application/vnd.github+json" }
        if (-not $sha) { $sha = $commit.sha }
        $when = ([datetime]$commit.commit.author.date).ToLocalTime().ToString("yyyy-MM-dd HH:mm")
        $subject = ($commit.commit.message -split "`n")[0]
    } catch {
        Write-Warn2 "Could not read commit metadata ($($_.Exception.Message)); downloading the ref archive instead."
    }

    $url = if ($sha) { "https://codeload.github.com/$Repo/zip/$sha" }
           else     { "https://codeload.github.com/$Repo/zip/refs/heads/$Ref" }
    $out = Join-Path $env:TEMP ("targetcard-" + ($(if ($sha) { $sha.Substring(0,8) } else { $Ref -replace '[^\w]','_' })) + ".zip")

    Write-Host "   downloading $url"
    Invoke-WebRequest -UseBasicParsing -Uri $url -OutFile $out -TimeoutSec 300
    $hash = (Get-FileHash $out -Algorithm SHA256).Hash

    if ($sha)    { Write-Ok ("commit:  " + $sha.Substring(0,12) + "   " + $when) }
    if ($subject) { Write-Ok ("subject: " + $subject) }
    Write-Ok ("archive: " + [math]::Round((Get-Item $out).Length / 1KB, 0) + " KB  sha256 " + $hash.Substring(0,16) + "...")

    return [pscustomobject]@{ Path = $out; Sha = $sha; Sha256 = $hash; Subject = $subject; When = $when }
}

function Set-BuildVersion {
    # Windows Installer only replaces an existing install when the version
    # differs: the same version opens the repair dialog. Upstream bumps its own
    # patch number a few times a day, which is exactly often enough to collide
    # with a rebuild, so every build gets its own number here. The fork allows
    # downgrades, so this only has to be *different*, never greater.
    $days = [math]::Floor(((Get-Date).ToUniversalTime() - [datetime]'2026-10-01T00:00:00Z').TotalDays)
    $build = 1000 + [int]$days * 40 + (Get-Date).ToUniversalTime().Hour
    $version = "2.0.$build"

    $conf = Join-Path $WorkDir "src-tauri\tauri.conf.json"
    $text = Get-Content -Raw $conf
    $upstream = "unknown"
    $found = [regex]::Match($text, '"version"\s*:\s*"([^"]+)"')
    if ($found.Success) { $upstream = $found.Groups[1].Value }
    $stamped = [regex]::Replace($text, '"version"\s*:\s*"[^"]+"', ('"version": "' + $version + '"'), 1)
    # [IO.File] keeps the file byte-for-byte apart from the version: no BOM, no
    # extra newline, which a JSON parser would forgive but a diff would not.
    if ($stamped -ne $text) { [IO.File]::WriteAllText($conf, $stamped) }

    # Same number in Cargo.toml, so the "Compiling ... vX" line names the build.
    $cargo = Join-Path $WorkDir "src-tauri\Cargo.toml"
    $cargoText = Get-Content -Raw $cargo
    $cargoStamped = [regex]::Replace($cargoText, '(?m)^version\s*=\s*"[^"]+"', ('version = "' + $version + '"'), 1)
    if ($cargoStamped -ne $cargoText) { [IO.File]::WriteAllText($cargo, $cargoStamped) }

    # And package.json, so npm's banner names the same build instead of the
    # upstream patch number the source was cut from.
    $pkg = Join-Path $WorkDir "package.json"
    if (Test-Path $pkg) {
        $pkgText = Get-Content -Raw $pkg
        $pkgStamped = [regex]::Replace($pkgText, '"version"\s*:\s*"[^"]+"', ('"version": "' + $version + '"'), 1)
        if ($pkgStamped -ne $pkgText) { [IO.File]::WriteAllText($pkg, $pkgStamped) }
    }

    return [pscustomobject]@{ Build = $version; Upstream = $upstream }
}

function Set-DiagnosticsCollector {
    # The token must not live in the repository (public, GPL fork), so it is
    # written to the app's own data directory by whoever runs the build, once.
    if (-not $CollectorToken) { return $false }
    $dir = Join-Path $env:APPDATA "com.targetcard.desktop"
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    $config = [ordered]@{
        upload_url    = $CollectorUrl
        token         = $CollectorToken
        interval_secs = 300
    }
    $path = Join-Path $dir "diagnostics.json"
    [IO.File]::WriteAllText($path, ($config | ConvertTo-Json))
    Write-Ok ("diagnostics: logs go to " + $CollectorUrl + " every 5 min")
    Write-Ok ("             " + $path)
    return $true
}

function Stop-RunningTargetcard {
    # msiexec cannot replace files a running app holds open.
    Get-Process -ErrorAction SilentlyContinue |
        Where-Object { $_.Path -like "*\targetcard\*" } |
        ForEach-Object {
            Write-Host "   stopping $($_.ProcessName) ($($_.Id))"
            Stop-Process -Id $_.Id -Force -ErrorAction SilentlyContinue
        }
    Start-Sleep -Milliseconds 400
}

function Get-InstalledTargetcard {
    $roots = @(
        "HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\*",
        "HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\*",
        "HKCU:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\*"
    )
    Get-ItemProperty $roots -ErrorAction SilentlyContinue |
        Where-Object { $_.DisplayName -like "targetcard*" -and $_.PSChildName -match '^\{' } |
        Select-Object -First 1
}

function Uninstall-Targetcard {
    Write-Step "Uninstall"
    $app = Get-InstalledTargetcard
    if (-not $app) { Write-Ok "nothing installed"; return }
    Write-Ok ("removing " + $app.DisplayName + " " + $app.DisplayVersion)
    Stop-RunningTargetcard
    $code = $app.PSChildName
    $log = Join-Path $env:TEMP "targetcard-uninstall.log"
    $proc = Start-Process msiexec.exe -ArgumentList "/x", $code, "/qb", "/norestart", "/l*v", "`"$log`"" -Wait -PassThru
    if ($proc.ExitCode -in 0, 3010) { Write-Ok "removed" }
    else { Write-Warn2 ("uninstall exit code " + $proc.ExitCode + "; log " + $log) }
}

function Install-Msi([string]$Msi) {
    Write-Step "Install"
    $installed = Get-InstalledTargetcard
    if ($installed) {
        Write-Ok ("upgrading " + $installed.DisplayName + " " + $installed.DisplayVersion)
        Stop-RunningTargetcard
    } else {
        Write-Ok "first install"
    }
    $log = Join-Path $env:TEMP "targetcard-install.log"
    $proc = Start-Process msiexec.exe `
        -ArgumentList "/i", "`"$Msi`"", "/qb", "/norestart", "/l*v", "`"$log`"" -Wait -PassThru
    if ($proc.ExitCode -in 0, 3010) {
        $now = Get-InstalledTargetcard
        if ($now) { Write-Ok ("installed " + $now.DisplayVersion) } else { Write-Ok "installed" }
    } else {
        Write-Warn2 ("install exit code " + $proc.ExitCode + "; log " + $log)
        Write-Warn2 "the MSI is still on disk, so it can be installed by hand"
    }
}

function Show-SourceFingerprint([string]$Zip, $Source, $Build) {
    # Unpack, then state what is actually on disk. This is the last chance to
    # catch a wrong or stale tree before twenty minutes of compiling.
    Write-Step "Source check"
    $hash = (Get-FileHash $Zip -Algorithm SHA256).Hash
    $cargo = Join-Path $WorkDir "src-tauri\Cargo.toml"
    $version = "unknown"
    if (Test-Path $cargo) {
        $match = Select-String -Path $cargo -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1
        if ($match) { $version = $match.Matches[0].Groups[1].Value }
    }
    $pkgVersion = "unknown"
    $pkg = Join-Path $WorkDir "package.json"
    if (Test-Path $pkg) {
        $m2 = Select-String -Path $pkg -Pattern '"version"\s*:\s*"([^"]+)"' | Select-Object -First 1
        if ($m2) { $pkgVersion = $m2.Matches[0].Groups[1].Value }
    }
    $cardFiles = @("public\targetcard.html", "src-tauri\src\targetcard\score_service.rs")
    $missing = @($cardFiles | Where-Object { -not (Test-Path (Join-Path $WorkDir $_)) })
    $card = if ($missing.Count -eq 0) { "present" } else { "MISSING" }

    $upstream = if ($Build) { $Build.Upstream } else { "?" }
    Write-Ok ("script:    " + $ScriptStamp)
    Write-Ok ("version:   " + $version + "  (upstream " + $upstream + ")   npm " + $pkgVersion)
    Write-Ok ("card code: " + $card)
    Write-Ok ("archive:   " + $hash.Substring(0, 16) + "...")
    if ($missing.Count -gt 0) {
        Write-Warn2 "This source predates the card window ($($missing -join ', '))."
        Write-Warn2 "Nothing here is worth building. Check the repo and ref."
    }
    if ($Source -and $Source.Sha) {
        $stamp = Join-Path $WorkDir ".last-build-commit"
        if ((Test-Path $stamp) -and ((Get-Content $stamp -Raw).Trim() -eq $Source.Sha)) {
            Write-Warn2 "Same commit as your last build. Nothing new upstream."
        }
        Set-Content -Path $stamp -Value $Source.Sha -NoNewline
    }
    return [pscustomobject]@{ Sha256 = $hash; Version = $version; NpmVersion = $pkgVersion; Card = $card }
}

function Expand-Source([string]$Zip) {
    Write-Step "Unpack"
    if ((Test-Path (Join-Path $WorkDir "package.json")) -and -not $Force) {
        Write-Ok "$WorkDir already has the source (use -Force to replace it)"
        return
    }

    # Extract into a staging folder, then copy over the working tree. Nothing is
    # moved out and back: a program holding a handle inside target (Explorer with
    # the MSI selected is enough) used to abort the whole build on the move,
    # before a single line was compiled.
    $stage = Join-Path $env:TEMP "targetcard-stage"
    if (Test-Path $stage) { Remove-Item $stage -Recurse -Force -ErrorAction SilentlyContinue }
    New-Item -ItemType Directory -Path $stage -Force | Out-Null
    Write-Host "   extracting $(Split-Path $Zip -Leaf)"
    [IO.Compression.ZipFile]::ExtractToDirectory($Zip, $stage)

    # GitHub archives put everything under one folder: `targetcard-main` or
    # `targetcard-<sha>`. Flatten it so paths match the docs.
    if (-not (Test-Path (Join-Path $stage "package.json"))) {
        $tops = @(Get-ChildItem $stage -Directory)
        if ($tops.Count -eq 1 -and (Test-Path (Join-Path $tops[0].FullName "package.json"))) {
            Get-ChildItem $tops[0].FullName -Force | Move-Item -Destination $stage -Force
            Remove-Item $tops[0].FullName -Recurse -Force
        }
    }
    if (-not (Test-Path (Join-Path $stage "package.json"))) { throw "unexpected archive layout: no package.json in the archive" }

    # Clear the old source, but never the two directories that cost real time:
    # src-tauri\target is 10-30 minutes of Rust, node_modules a few hundred MB.
    New-Item -ItemType Directory -Path $WorkDir -Force | Out-Null
    Get-ChildItem $WorkDir -Force |
        Where-Object { $_.Name -ne "node_modules" -and $_.Name -ne "src-tauri" } |
        ForEach-Object { Remove-Item $_.FullName -Recurse -Force -ErrorAction SilentlyContinue }
    $src = Join-Path $WorkDir "src-tauri"
    if (Test-Path $src) {
        Get-ChildItem $src -Force |
            Where-Object { $_.Name -ne "target" } |
            ForEach-Object { Remove-Item $_.FullName -Recurse -Force -ErrorAction SilentlyContinue }
    }

    Write-Host "   applying source over $WorkDir (build output kept)"
    Copy-Item -Path (Join-Path $stage "*") -Destination $WorkDir -Recurse -Force
    Remove-Item $stage -Recurse -Force -ErrorAction SilentlyContinue

    if (-not (Test-Path (Join-Path $WorkDir "package.json"))) { throw "unexpected archive layout: no package.json in $WorkDir" }
    Write-Ok "source at $WorkDir"
}

Assert-Admin

if ($Uninstall) {
    Uninstall-Targetcard
    return
}

if ($BuildOnly) {
    if (-not (Test-Cargo))  { throw "-BuildOnly needs cargo on PATH" }
    if (-not (Test-Node))   { throw "-BuildOnly needs node on PATH" }
} else {
    Write-Step "Toolchain check"
    Write-Host "   cargo:  $(if (Test-Cargo)   {'present'} else {'missing -> install'})"
    Write-Host "   MSVC:   $(if (Test-VcTools) {'present'} else {'missing -> install'})"
    Write-Host "   node:   $(if (Test-Node)    {'present'} else {'missing -> install'})"
    Write-Host "   WebView2: $(if (Test-WebView2) {'present'} else {'missing -> install'})"
    if (-not (Test-Path "C:\Windows\System32\Npcap") -and -not (Test-Path "C:\Program Files\Wireshark")) {
        Write-Warn2 "Npcap not found. The app needs it to capture packets: install Wireshark or Npcap first."
    }
    Write-Host "`n   Installs can take 30-60 minutes the first time and use ~8 GB." -ForegroundColor Yellow
    if (-not $PSBoundParameters.ContainsKey("SetupOnly")) {
        Read-Host "   Press Enter to continue, or Ctrl+C to abort" | Out-Null
    }
    Install-Rust
    Install-VcTools
    Install-Node
    Install-WebView2
    if ($SetupOnly) { Write-Host "`nToolchain ready. Re-run with -BuildOnly to build." -ForegroundColor Green; return }
}

# Local archive if given, otherwise the latest commit from GitHub.
if ($ZipPath) {
    if (-not (Test-Path $ZipPath)) { throw "no such file: $ZipPath" }
    Write-Step "Source download"
    Write-Ok ("local archive: " + (Resolve-Path $ZipPath).Path)
    $source = [pscustomobject]@{ Path = (Resolve-Path $ZipPath).Path; Sha = $null; Sha256 = (Get-FileHash $ZipPath -Algorithm SHA256).Hash }
} else {
    $source = Get-SourceArchive
}

Expand-Source $source.Path
$buildVersion = Set-BuildVersion
$fingerprint = Show-SourceFingerprint $source.Path $source $buildVersion

Write-Step "npm ci"
Push-Location $WorkDir
try {
    & npm ci --no-audit --no-fund
    if ($LASTEXITCODE -ne 0) {
        Write-Warn2 "npm ci failed; trying npm install"
        & npm install --no-audit --no-fund
        if ($LASTEXITCODE -ne 0) { throw "npm install failed with exit code $LASTEXITCODE" }
    }
    Write-Ok "dependencies installed"

    Write-Step "tauri build (offline: no uploads, no telemetry)"
    Write-Host "   first build takes 10-30 minutes; Rust compiles the whole app"
    & npm run build:offline
    if ($LASTEXITCODE -ne 0) { throw "build failed with exit code $LASTEXITCODE" }
} finally {
    Pop-Location
}

Write-Step "Result"
$msi = Get-ChildItem (Join-Path $WorkDir "src-tauri\target\release\bundle\msi\*.msi") -ErrorAction SilentlyContinue |
       Sort-Object LastWriteTime -Descending | Select-Object -First 1
if (-not $msi) { throw "build finished but no MSI was produced; check the output above" }
$hash = (Get-FileHash $msi.FullName -Algorithm SHA256).Hash
Write-Ok ("MSI:    " + $msi.FullName)
Write-Ok ("size:   " + [math]::Round($msi.Length / 1MB, 1) + " MB")
Write-Ok ("sha256: " + $hash)
$built = "version " + $fingerprint.Version + ", card code " + $fingerprint.Card
if ($source.Sha) { $built = "commit " + $source.Sha.Substring(0, 12) + ", " + $built }
Write-Host ("`n   Built " + $built) -ForegroundColor Yellow
Write-Host "   The build is unsigned, so SmartScreen will warn on first run." -ForegroundColor Yellow
Write-Host "   Install Npcap (Wireshark) if you have not, then run the app as Administrator." -ForegroundColor Yellow
if ($fingerprint.Card -ne "present") {
    Write-Host "   That build has no target card in it." -ForegroundColor Red
}

if ($NoInstall) {
    Write-Host "`n   -NoInstall given; the MSI is left on disk." -ForegroundColor Yellow
} else {
    Install-Msi $msi.FullName
}

if (-not (Set-DiagnosticsCollector)) {
    Write-Host "   diagnostics: no -CollectorToken given, the app uploads nothing." -ForegroundColor Yellow
}
