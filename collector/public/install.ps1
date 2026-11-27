# Installs Pathfinder on Windows, from PowerShell:
#   irm https://pathfinder.lprsrts.com/install.ps1 | iex
#
# It downloads the newest release for this computer, checks it against the
# release manifest's SHA-256, and puts `pathfinder.exe` in a folder for your
# own programs: PATHFINDER_BIN_DIR if set, else
# %LOCALAPPDATA%\Programs\Pathfinder, which it adds to your user PATH.
# Nothing else changes.

& {
    $ErrorActionPreference = 'Stop'
    $ProgressPreference = 'SilentlyContinue'
    # Windows PowerShell 5.1 can default to older TLS.
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

    $base = if ($env:PATHFINDER_URL) { $env:PATHFINDER_URL } else { 'https://pathfinder.lprsrts.com' }
    # The same release files on GitHub, used when the server can't be reached.
    $mirror = if ($env:PATHFINDER_MIRROR) { $env:PATHFINDER_MIRROR } else { 'https://github.com/lprsrts/pathfinder/releases' }
    $dest = if ($env:PATHFINDER_BIN_DIR) {
        $env:PATHFINDER_BIN_DIR
    } elseif ($env:LOCALAPPDATA) {
        Join-Path $env:LOCALAPPDATA 'Programs\Pathfinder'
    } else {
        Write-Error "LOCALAPPDATA isn't set; say where to install with `$env:PATHFINDER_BIN_DIR = 'C:\some\folder'."
        return
    }

    # The operating system's architecture, not this PowerShell's: an x64
    # PowerShell on an Arm PC still gets the Arm build.
    $arch = $null
    try { $arch = [string][System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture } catch {}
    if (-not $arch) {
        $arch = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }
    }
    $target = switch -Regex ($arch) {
        '^(X64|AMD64)$' { 'x86_64-pc-windows-msvc' }
        '^(Arm64|ARM64)$' { 'aarch64-pc-windows-msvc' }
        default { $null }
    }
    if (-not $target) {
        Write-Error "Sorry, there's no Pathfinder build for Windows on $arch yet."
        return
    }

    $tmp = Join-Path ([IO.Path]::GetTempPath()) ("pathfinder-install-" + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $tmp | Out-Null
    try {
        $minute = [int][Math]::Floor(([DateTimeOffset]::UtcNow.ToUnixTimeSeconds()) / 60)
        try {
            $manifest = Invoke-RestMethod -UseBasicParsing "$base/releases/latest.json?m=$minute"
        } catch {
            Write-Host "Pathfinder's server didn't answer; using the release on GitHub."
            $manifest = Invoke-RestMethod -UseBasicParsing "$mirror/latest/download/latest.json"
        }
        $version = $manifest.version
        $entry = $manifest.files.$target
        if (-not $entry -or -not $entry.path -or -not $entry.sha256) {
            Write-Error "Sorry, release $version has no build for this computer ($target)."
            return
        }

        Write-Host "Downloading Pathfinder $version..."
        $archive = Join-Path $tmp 'release.tar.gz'
        try {
            Invoke-WebRequest -UseBasicParsing "$base/releases/$($entry.path)" -OutFile $archive
        } catch {
            $name = Split-Path -Leaf $entry.path
            Invoke-WebRequest -UseBasicParsing "$mirror/download/v$version/$name" -OutFile $archive
        }
        $got = (Get-FileHash -Algorithm SHA256 -Path $archive).Hash
        if ($got -ne $entry.sha256) {
            Write-Error "The download doesn't match the release manifest; stopping."
            return
        }
        $unpacked = Join-Path $tmp 'release'
        New-Item -ItemType Directory -Path $unpacked | Out-Null
        # Windows 10 (1803) and later ship tar.exe, which reads .tar.gz.
        $tar = Join-Path $env:SystemRoot 'System32\tar.exe'
        if (-not (Test-Path -LiteralPath $tar -PathType Leaf)) { $tar = 'tar.exe' }
        & $tar -xzf $archive -C $unpacked pathfinder.exe
        if ($LASTEXITCODE -ne 0) {
            Write-Error "Couldn't unpack the release; stopping."
            return
        }
        $prog = Join-Path $unpacked 'pathfinder.exe'
        if (-not (Test-Path -LiteralPath $prog -PathType Leaf)) {
            Write-Error "The release has no pathfinder.exe; stopping."
            return
        }

        New-Item -ItemType Directory -Force -Path $dest | Out-Null
        $exe = Join-Path $dest 'pathfinder.exe'
        # A running program can't be replaced, but it can be renamed.
        $old = Join-Path $dest 'pathfinder.exe.old'
        Remove-Item -LiteralPath $old -Force -ErrorAction SilentlyContinue
        if (Test-Path -LiteralPath $exe) { Move-Item -LiteralPath $exe -Destination $old -Force }
        Move-Item -LiteralPath $prog -Destination $exe -Force
        Write-Host "Installed: $exe ($version)"
    } finally {
        Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue
    }

    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    $parts = @($userPath -split ';' | Where-Object { $_ })
    if ($parts -notcontains $dest) {
        [Environment]::SetEnvironmentVariable('Path', (($parts + $dest) -join ';'), 'User')
        $env:Path = "$env:Path;$dest"
        Write-Host ""
        Write-Host "Added $dest to your user PATH. Open a new terminal for other windows to see it."
    } else {
        $found = Get-Command pathfinder -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
        if ($found -and $found.Source -ne $exe) {
            Write-Host ""
            Write-Host "Note: another pathfinder comes first on your PATH ($($found.Source))."
            Write-Host "Remove it, or run Pathfinder as $exe."
        }
    }

    Write-Host @'

Next:
  1. Connect your model account (opens your browser):
                         pathfinder auth login chatgpt
  2. Start it in the folder you're working in:
                         cd ~\path\to\your\work; pathfinder
     The first start asks whether to join the beta (with your email) and,
     separately, whether to share usage data. Both are optional.
  3. Later updates:      pathfinder update
  Problems:              /bug in Pathfinder, or pathfinder bug "what happened"

Note: Windows has no sandbox for the agent's commands, so Pathfinder shows
you every command and runs it only once you approve it.

Privacy notice: https://pathfinder.lprsrts.com/privacy/
'@
}
