# ==============================================================================
#  MQ Launcher -- 1-Command Universal Windows Installer
#  Usage:
#    irm https://github.com/MrQuster/mq/releases/latest/download/install.ps1 | iex
#  Advanced:
#    & ([scriptblock]::Create((irm https://github.com/MrQuster/mq/releases/latest/download/install.ps1))) -Portable
#    & ([scriptblock]::Create((irm https://github.com/MrQuster/mq/releases/latest/download/install.ps1))) -Uninstall
#    & ([scriptblock]::Create((irm https://github.com/MrQuster/mq/releases/latest/download/install.ps1))) -Version 2.7.4 -Silent
# ==============================================================================

function Invoke-MqInstaller {
    [CmdletBinding()]
    param(
        [Parameter(Position = 0)]
        [string]$Version = "",

        [switch]$Portable,

        [switch]$Silent,

        [switch]$Uninstall,

        [switch]$Purge,

        [string]$InstallDir = "",

        [string]$BaseUrl = "",

        [switch]$NoDesktopShortcut,

        [switch]$Force,

        [switch]$Help
    )

    $ErrorActionPreference = "Stop"
    Set-StrictMode -Version Latest

    # Fallback parameter bindings via environment variables for `irm ... | iex`
    if (-not $Version) { $Version = [Environment]::GetEnvironmentVariable("MQ_VERSION") }
    if (-not $Version) { $Version = "latest" }

    $envPortable = [Environment]::GetEnvironmentVariable("MQ_PORTABLE")
    if (-not $Portable -and ($envPortable -eq "1" -or $envPortable -ieq "true")) { $Portable = $true }

    $envSilent = [Environment]::GetEnvironmentVariable("MQ_SILENT")
    if (-not $Silent -and ($envSilent -eq "1" -or $envSilent -ieq "true")) { $Silent = $true }

    $envUninstall = [Environment]::GetEnvironmentVariable("MQ_UNINSTALL")
    if (-not $Uninstall -and ($envUninstall -eq "1" -or $envUninstall -ieq "true")) { $Uninstall = $true }

    $envPurge = [Environment]::GetEnvironmentVariable("MQ_PURGE")
    if (-not $Purge -and ($envPurge -eq "1" -or $envPurge -ieq "true")) { $Purge = $true }

    $envNoDesktop = [Environment]::GetEnvironmentVariable("MQ_NO_DESKTOP_SHORTCUT")
    if (-not $NoDesktopShortcut -and ($envNoDesktop -eq "1" -or $envNoDesktop -ieq "true")) { $NoDesktopShortcut = $true }

    $envForce = [Environment]::GetEnvironmentVariable("MQ_FORCE")
    if (-not $Force -and ($envForce -eq "1" -or $envForce -ieq "true")) { $Force = $true }

    if (-not $InstallDir) { $InstallDir = [Environment]::GetEnvironmentVariable("MQ_INSTALL_DIR") }
    if (-not $BaseUrl) { $BaseUrl = [Environment]::GetEnvironmentVariable("MQ_BASE_URL") }

    if ($Help) {
        Write-Host @"
MQ Launcher -- Windows Installer
Usage:
  install.ps1 [-Version <tag>] [-Portable] [-Silent] [-Uninstall] [-Purge] [-InstallDir <path>] [-BaseUrl <url>] [-NoDesktopShortcut] [-Force]

Parameters:
  -Version <tag>       Specify version to install (e.g. '2.7.4' or 'latest'). Default: latest
  -Portable            Install standalone portable package into %LOCALAPPDATA%\MQ-Portable
  -Silent              Run unattended installation (/VERYSILENT /SUPPRESSMSGBOXES /NORESTART)
  -Uninstall           Silently uninstall MQ Launcher (preserves %LOCALAPPDATA%\MQ user data)
  -Purge               With -Uninstall, also remove all user data, instances, mods and settings
  -InstallDir <path>   Custom installation target directory
  -BaseUrl <url>       Override release download base URL (for local/CI testing)
  -NoDesktopShortcut   Do not create a shortcut on Desktop
  -Force               Force-close active running instances immediately
"@ -ForegroundColor Cyan
        return
    }

    # 1. Verify Windows 10+ x64 platform
    if (-not [Environment]::Is64BitOperatingSystem) {
        throw "MQ Launcher requires 64-bit (x64) Windows."
    }
    $osVersion = [System.Environment]::OSVersion.Version
    if ($osVersion.Major -lt 10) {
        throw "MQ Launcher requires Windows 10 or newer (detected $($osVersion.ToString()))."
    }

    # 2. Enforce modern secure TLS protocols (TLS 1.2, plus TLS 1.3 when supported by .NET)
    try {
        [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
        if ([Enum]::IsDefined([Net.SecurityProtocolType], "Tls13")) {
            [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls13
        }
    } catch {
        try {
            [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor 3072
        } catch {}
        try {
            [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor 12288
        } catch {}
    }

    # Repository & Minisign configuration
    $envRepo = [Environment]::GetEnvironmentVariable("MQ_REPO")
    $Repo = if ($envRepo) { $envRepo } else { "MrQuster/mq" }
    $MinisignPubKey = "RWRlxrUOJgeLq7AIndJRG9ZhsdT5QiHy9usykXvxt1C49a1h/km3h9lt"
    $AppName = "MQ Launcher"
    $InnoAppIdKey = "{9D7C52A4-325A-4B6A-B68C-7C4E15F59D18}_is1"

    $LocalAppDataDir = [Environment]::GetFolderPath("LocalApplicationData")
    if ([string]::IsNullOrWhiteSpace($LocalAppDataDir)) {
        $LocalAppDataDir = [Environment]::GetEnvironmentVariable("LOCALAPPDATA")
    }
    $RoamingAppDataDir = [Environment]::GetFolderPath("ApplicationData")
    if ([string]::IsNullOrWhiteSpace($RoamingAppDataDir)) {
        $RoamingAppDataDir = [Environment]::GetEnvironmentVariable("APPDATA")
    }

    $DefaultInstallDir = Join-Path $LocalAppDataDir "Programs\MQ"
    $DefaultPortableDir = Join-Path $LocalAppDataDir "MQ-Portable"
    $UserDataDir = Join-Path $LocalAppDataDir "MQ"

    if (-not $Silent) {
        Write-Host "================================================================================" -ForegroundColor Cyan
        Write-Host "        MQ Launcher -- Windows Installer                                        " -ForegroundColor Cyan
        Write-Host "================================================================================" -ForegroundColor Cyan
    }

    function Test-LocalPath {
        param(
            [string]$Path,
            [ValidateSet("Any", "Leaf", "Container")]
            [string]$PathType = "Any"
        )
        if ([string]::IsNullOrWhiteSpace($Path)) {
            return $false
        }
        if ($Path -match '^[a-zA-Z][a-zA-Z0-9+.-]*://') {
            return $false
        }
        try {
            return (Test-Path -LiteralPath $Path -PathType $PathType -ErrorAction SilentlyContinue)
        } catch {
            return $false
        }
    }

    function Normalize-DirKey {
        param([string]$Path)
        if ([string]::IsNullOrWhiteSpace($Path)) { return "" }
        return $Path.Trim().TrimEnd('\', '/').ToLowerInvariant()
    }

    function Get-UninstallerExePath {
        param([string]$UninstallString)
        if ([string]::IsNullOrWhiteSpace($UninstallString)) {
            return ""
        }
        $trimmed = $UninstallString.Trim()
        if ($trimmed -match '^"([^"]+)"') {
            return $matches[1]
        }
        if ($trimmed -match '^([^\s]+\.exe)\b') {
            return $matches[1]
        }
        return $trimmed.Trim('"')
    }

    function Add-UserPathEntry {
        param([Parameter(Mandatory = $true)][string]$DirPath)
        $cleanDir = $DirPath.Trim().TrimEnd('\')
        if ([string]::IsNullOrWhiteSpace($cleanDir)) { return }
        $targetKey = Normalize-DirKey $cleanDir

        # Update persistent User PATH without corrupting REG_EXPAND_SZ entries or empty PATH
        try {
            $envKey = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey("Environment", $true)
            if ($null -ne $envKey) {
                try {
                    $rawUserPath = $envKey.GetValue("Path", "", [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
                    if ($null -eq $rawUserPath) { $rawUserPath = "" }
                    $parts = New-Object System.Collections.Generic.List[string]
                    $seen = @{}
                    foreach ($entry in ([string]$rawUserPath -split ';')) {
                        $trimmed = $entry.Trim()
                        if (-not [string]::IsNullOrWhiteSpace($trimmed)) {
                            $k = Normalize-DirKey $trimmed
                            if (-not $seen.ContainsKey($k)) {
                                $seen[$k] = $true
                                $parts.Add($trimmed)
                            }
                        }
                    }
                    if (-not $seen.ContainsKey($targetKey)) {
                        $parts.Add($cleanDir)
                        $newUserPath = ($parts.ToArray() -join ';')
                        if ($newUserPath -like '*%*') {
                            $envKey.SetValue("Path", $newUserPath, [Microsoft.Win32.RegistryValueKind]::ExpandString)
                        } else {
                            [Environment]::SetEnvironmentVariable("Path", $newUserPath, "User")
                        }
                    }
                } finally {
                    $envKey.Close()
                }
            }
        } catch {}

        # Update current PowerShell session PATH so `mq` works immediately
        $sessionPath = [Environment]::GetEnvironmentVariable("Path", "Process")
        if ($null -eq $sessionPath) { $sessionPath = "" }
        $sessionHas = $false
        foreach ($entry in ($sessionPath -split ';')) {
            if ((Normalize-DirKey $entry) -eq $targetKey) {
                $sessionHas = $true
                break
            }
        }
        if (-not $sessionHas) {
            $newSessionPath = if ([string]::IsNullOrWhiteSpace($sessionPath)) {
                $cleanDir
            } else {
                "$($sessionPath.TrimEnd(';'));$cleanDir"
            }
            [Environment]::SetEnvironmentVariable("Path", $newSessionPath, "Process")
        }
    }

    function Remove-UserPathEntries {
        param([string[]]$Directories)
        $removeKeys = @{}
        foreach ($d in $Directories) {
            $k = Normalize-DirKey $d
            if ($k) { $removeKeys[$k] = $true }
        }
        if ($removeKeys.Count -eq 0) { return }

        try {
            $envKey = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey("Environment", $true)
            if ($null -ne $envKey) {
                try {
                    $rawUserPath = $envKey.GetValue("Path", $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
                    if (-not [string]::IsNullOrWhiteSpace([string]$rawUserPath)) {
                        $kept = New-Object System.Collections.Generic.List[string]
                        $changed = $false
                        foreach ($entry in ([string]$rawUserPath -split ';')) {
                            $trimmed = $entry.Trim()
                            if ([string]::IsNullOrWhiteSpace($trimmed)) { continue }
                            if ($removeKeys.ContainsKey((Normalize-DirKey $trimmed))) {
                                $changed = $true
                            } else {
                                $kept.Add($trimmed)
                            }
                        }
                        if ($changed) {
                            $newUserPath = ($kept.ToArray() -join ';')
                            if ($newUserPath -like '*%*') {
                                $envKey.SetValue("Path", $newUserPath, [Microsoft.Win32.RegistryValueKind]::ExpandString)
                            } else {
                                [Environment]::SetEnvironmentVariable("Path", $newUserPath, "User")
                            }
                        }
                    }
                } finally {
                    $envKey.Close()
                }
            }
        } catch {}

        # Also clean current session PATH
        $sessionPath = [Environment]::GetEnvironmentVariable("Path", "Process")
        if (-not [string]::IsNullOrWhiteSpace($sessionPath)) {
            $keptSession = @($sessionPath -split ';' | Where-Object {
                $t = $_.Trim()
                $t -and (-not $removeKeys.ContainsKey((Normalize-DirKey $t)))
            })
            [Environment]::SetEnvironmentVariable("Path", ($keptSession -join ';'), "Process")
        }
    }

    # Helper function to close running instances of mq.exe only (never touches java.exe/javaw.exe)
    function Stop-RunningMQ {
        $processes = @(Get-Process -Name "mq" -ErrorAction SilentlyContinue)
        if ($processes.Count -eq 0) {
            return
        }
        if (-not $Silent) {
            Write-Host "Closing running instances of $AppName..." -ForegroundColor Yellow
        }
        if (-not $Force) {
            foreach ($proc in $processes) {
                try {
                    if ($proc.MainWindowHandle -ne [IntPtr]::Zero) {
                        $null = $proc.CloseMainWindow()
                    }
                } catch {}
            }
            for ($wait = 0; $wait -lt 15; $wait++) {
                $alive = @(Get-Process -Name "mq" -ErrorAction SilentlyContinue)
                if ($alive.Count -eq 0) { return }
                Start-Sleep -Milliseconds 100
            }
        }
        $remaining = @(Get-Process -Name "mq" -ErrorAction SilentlyContinue)
        foreach ($proc in $remaining) {
            try {
                Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
            } catch {}
        }
        for ($wait = 0; $wait -lt 30; $wait++) {
            $alive = @(Get-Process -Name "mq" -ErrorAction SilentlyContinue)
            if ($alive.Count -eq 0) { break }
            Start-Sleep -Milliseconds 100
        }
    }

    # Helper to download file safely from HTTP(S) or local file path
    function Get-ReleaseFile {
        param(
            [Parameter(Mandatory = $true)][string]$Url,
            [Parameter(Mandatory = $true)][string]$Destination
        )
        if (Test-LocalPath -Path $Url -PathType Leaf) {
            Copy-Item -LiteralPath $Url -Destination $Destination -Force
            return
        }
        $prevProgress = $ProgressPreference
        $ProgressPreference = "SilentlyContinue"
        try {
            try {
                Invoke-WebRequest -Uri $Url -OutFile $Destination -UseBasicParsing -MaximumRedirection 10 -Headers @{ "User-Agent" = "MQ-Installer/1.0" }
            } catch {
                $wc = New-Object System.Net.WebClient
                try {
                    $wc.Headers.Add("User-Agent", "MQ-Installer/1.0")
                    $wc.DownloadFile($Url, $Destination)
                } finally {
                    $wc.Dispose()
                }
            }
        } finally {
            $ProgressPreference = $prevProgress
        }
    }

    # Helper to verify Minisign signature of SHA256SUMS when minisign is available
    function Test-MinisignSignature {
        param(
            [Parameter(Mandatory = $true)][string]$FilePath,
            [Parameter(Mandatory = $true)][string]$SigUrl
        )
        $minisignCmd = Get-Command "minisign" -ErrorAction SilentlyContinue
        if (-not $minisignCmd) {
            return
        }
        $sigPath = "$FilePath.minisig"
        try {
            Get-ReleaseFile -Url $SigUrl -Destination $sigPath
        } catch {
            if ($BaseUrl) {
                # Local test servers may not provide .minisig files
                return
            }
            throw "Failed to download Minisign signature from $SigUrl : $_"
        }
        $global:LASTEXITCODE = 0
        & $minisignCmd.Source -Vm $FilePath -P $MinisignPubKey -x $sigPath | Out-Null
        if ($LASTEXITCODE -ne 0) {
            throw "Minisign signature verification failed for $(Split-Path $FilePath -Leaf)!"
        }
        if (-not $Silent) {
            Write-Host "[OK] Minisign signature verified" -ForegroundColor Green
        }
    }

    # ------------------------------------------------------------------------------
    # Action: UNINSTALL
    # ------------------------------------------------------------------------------
    if ($Uninstall) {
        if (-not $Silent) {
            Write-Host "[1/2] Searching for existing installation of $AppName..." -ForegroundColor Cyan
        }

        Stop-RunningMQ

        $uninstalled = $false
        $uninstArgs = @("/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART")
        if ($Purge) {
            $uninstArgs += "/PURGE=1"
        }

        # Prefer default per-user uninstaller (%LOCALAPPDATA%\Programs\MQ\unins000.exe)
        $defaultUnins = Join-Path $DefaultInstallDir "unins000.exe"
        if (Test-Path -LiteralPath $defaultUnins) {
            if (-not $Silent) {
                Write-Host "Running uninstaller: $defaultUnins" -ForegroundColor Yellow
            }
            $proc = Start-Process -FilePath $defaultUnins -ArgumentList $uninstArgs -Wait -PassThru -NoNewWindow
            if ($proc.ExitCode -ne 0) {
                throw "Uninstaller failed with exit code $($proc.ExitCode)"
            }
            $uninstalled = $true
        }

        # Check Windows Registry by exact Inno Setup AppId first, then MrQuster Publisher match
        if (-not $uninstalled) {
            $exactRegPaths = @(
                "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\$InnoAppIdKey",
                "HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\$InnoAppIdKey"
            )
            foreach ($exactPath in $exactRegPaths) {
                if (Test-Path -LiteralPath $exactPath) {
                    $props = Get-ItemProperty -LiteralPath $exactPath -ErrorAction SilentlyContinue
                    if ($null -ne $props -and $props.PSObject.Properties['UninstallString'] -and $props.UninstallString) {
                        $rawCmd = Get-UninstallerExePath ([string]$props.UninstallString)
                        if ($rawCmd -and (Test-Path -LiteralPath $rawCmd)) {
                            if (-not $Silent) {
                                Write-Host "Running uninstaller: $rawCmd" -ForegroundColor Yellow
                            }
                            $proc = Start-Process -FilePath $rawCmd -ArgumentList $uninstArgs -Wait -PassThru -NoNewWindow
                            if ($proc.ExitCode -ne 0) {
                                throw "Uninstaller failed with exit code $($proc.ExitCode)"
                            }
                            $uninstalled = $true
                            break
                        }
                    }
                }
            }
        }

        if (-not $uninstalled) {
            $registryKeys = @(
                "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*",
                "HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*"
            )
            foreach ($regKey in $registryKeys) {
                $matchesFound = @(Get-ItemProperty -Path $regKey -ErrorAction SilentlyContinue |
                    Where-Object {
                        ($_.PSObject.Properties['Publisher']) -and ($_.Publisher -eq "MrQuster") -and
                        ($_.PSObject.Properties['DisplayName']) -and ($_.DisplayName -like "MQ*") -and
                        ($_.PSObject.Properties['UninstallString']) -and $_.UninstallString
                    })

                if ($matchesFound.Count -gt 0) {
                    $rawCmd = Get-UninstallerExePath ([string]$matchesFound[0].UninstallString)
                    if ($rawCmd -and (Test-Path -LiteralPath $rawCmd)) {
                        if (-not $Silent) {
                            Write-Host "Running uninstaller: $rawCmd" -ForegroundColor Yellow
                        }
                        $proc = Start-Process -FilePath $rawCmd -ArgumentList $uninstArgs -Wait -PassThru -NoNewWindow
                        if ($proc.ExitCode -ne 0) {
                            throw "Uninstaller failed with exit code $($proc.ExitCode)"
                        }
                        $uninstalled = $true
                        break
                    }
                }
            }
        }

        # Remove leftover Inno Setup / portable installation directories if present
        $installCandidates = @($DefaultInstallDir)
        $portableDir = if ($InstallDir) { $InstallDir } else { $DefaultPortableDir }
        if ($portableDir -and ($installCandidates -notcontains $portableDir)) {
            $installCandidates += $portableDir
        }
        foreach ($dirPath in $installCandidates) {
            if ($dirPath -and (Test-Path -LiteralPath $dirPath)) {
                if (-not $Silent) {
                    Write-Host "Removing installation directory $dirPath..." -ForegroundColor Yellow
                }
                Remove-Item -LiteralPath $dirPath -Recurse -Force -ErrorAction SilentlyContinue
                $uninstalled = $true
            }
        }

        # Clean up leftover shortcuts if any (uses Shell Folders for OneDrive/Cyrillic support)
        $shortcuts = New-Object System.Collections.Generic.List[string]
        $programsDir = [Environment]::GetFolderPath("Programs")
        if (-not [string]::IsNullOrWhiteSpace($programsDir)) {
            $shortcuts.Add((Join-Path $programsDir "MQ.lnk"))
            $shortcuts.Add((Join-Path $programsDir "MQ Launcher.lnk"))
        }
        $desktopDir = [Environment]::GetFolderPath("Desktop")
        if (-not [string]::IsNullOrWhiteSpace($desktopDir)) {
            $shortcuts.Add((Join-Path $desktopDir "MQ.lnk"))
            $shortcuts.Add((Join-Path $desktopDir "MQ Launcher.lnk"))
        }
        foreach ($sc in $shortcuts) {
            if (Test-Path -LiteralPath $sc) {
                Remove-Item -LiteralPath $sc -Force -ErrorAction SilentlyContinue
            }
        }

        # Clean MQ install directories from User PATH
        Remove-UserPathEntries -Directories $installCandidates

        # Purge user data if -Purge / MQ_PURGE=1
        if ($Purge) {
            $purgeTargets = @($UserDataDir)
            if (-not [string]::IsNullOrWhiteSpace($RoamingAppDataDir)) {
                $purgeTargets += (Join-Path $RoamingAppDataDir "MQ")
            }
            foreach ($d in $purgeTargets) {
                if ($d -and (Test-Path -LiteralPath $d)) {
                    if (-not $Silent) {
                        Write-Host "Purging user data directory $d..." -ForegroundColor Yellow
                    }
                    Remove-Item -LiteralPath $d -Recurse -Force -ErrorAction SilentlyContinue
                }
            }
        }

        if (-not $Silent) {
            Write-Host "`n[2/2] Uninstall complete!" -ForegroundColor Green
            if ($Purge) {
                Write-Host "User game instances, worlds and settings have been removed (-Purge)." -ForegroundColor Gray
            } else {
                Write-Host "User game instances and account data preserved at: $UserDataDir" -ForegroundColor Gray
            }
        }
        return
    }

    # ------------------------------------------------------------------------------
    # Action: INSTALL (Setup or Portable)
    # ------------------------------------------------------------------------------
    if ($BaseUrl) {
        $BaseDownloadUrl = $BaseUrl.TrimEnd('/', '\')
    } elseif ($Version -eq "latest") {
        $BaseDownloadUrl = "https://github.com/$Repo/releases/latest/download"
    } else {
        $tag = if ($Version.StartsWith("v")) { $Version } else { "v$Version" }
        $BaseDownloadUrl = "https://github.com/$Repo/releases/download/$tag"
    }

    $TempDir = Join-Path ([System.IO.Path]::GetTempPath()) ("mq-install-" + [System.Guid]::NewGuid().ToString("N"))
    New-Item -ItemType Directory -Path $TempDir -Force | Out-Null

    try {
        # Step 1: Download, verify (if minisign is present), and parse SHA256SUMS
        if (-not $Silent) {
            Write-Host "[1/4] Fetching SHA256SUMS ($Version)..." -ForegroundColor Cyan
        }
        $SumsFile = Join-Path $TempDir "SHA256SUMS"
        $SumsUrl = if (Test-LocalPath -Path $BaseDownloadUrl -PathType Container) {
            Join-Path $BaseDownloadUrl "SHA256SUMS"
        } else {
            "$BaseDownloadUrl/SHA256SUMS"
        }

        Get-ReleaseFile -Url $SumsUrl -Destination $SumsFile

        $sumsSigUrl = if (Test-LocalPath -Path $BaseDownloadUrl -PathType Container) {
            Join-Path $BaseDownloadUrl "SHA256SUMS.minisig"
        } else {
            "$BaseDownloadUrl/SHA256SUMS.minisig"
        }
        Test-MinisignSignature -FilePath $SumsFile -SigUrl $sumsSigUrl

        $checksums = @{}
        Get-Content -LiteralPath $SumsFile | ForEach-Object {
            if ($null -eq $_) { return }
            $line = ([string]$_).Trim().TrimStart([char]0xFEFF)
            if ($line -and -not $line.StartsWith("#")) {
                if ($line -match '^([a-fA-F0-9]{64})\s+\*?(.+)$') {
                    $hash = $matches[1].ToLowerInvariant()
                    $rawName = $matches[2].Trim()
                    $fname = [System.IO.Path]::GetFileName($rawName)
                    if ($fname) {
                        $checksums[$fname] = $hash
                    }
                }
            }
        }

        if ($checksums.Count -eq 0) {
            throw "SHA256SUMS is empty or invalid."
        }

        # Step 2: Determine target artifact name & download
        if (-not $Silent) {
            Write-Host "[2/4] Downloading $AppName payload..." -ForegroundColor Cyan
        }

        if ($Portable) {
            $artifactName = "mq-windows-x86_64-portable.zip"
            if (-not $checksums.ContainsKey($artifactName)) {
                foreach ($k in $checksums.Keys) {
                    if ($k -match '(?i)^mq-.*portable.*\.zip$') {
                        $artifactName = $k
                        break
                    }
                }
            }
        } else {
            $verClean = $Version.TrimStart('v')
            $artifactName = if ($Version -eq "latest") { "MQ-Setup.exe" } else { "MQ-Setup-$verClean-x64.exe" }
            if (-not $checksums.ContainsKey($artifactName)) {
                foreach ($k in $checksums.Keys) {
                    if ($k -match '(?i)^MQ-Setup.*-x64\.exe$') {
                        $artifactName = $k
                        break
                    }
                }
            }
            if (-not $checksums.ContainsKey($artifactName)) {
                foreach ($k in $checksums.Keys) {
                    if ($k -match '(?i)^MQ-Setup.*\.exe$') {
                        $artifactName = $k
                        break
                    }
                }
            }
        }

        if (-not $checksums.ContainsKey($artifactName)) {
            $available = ($checksums.Keys | Sort-Object) -join ", "
            throw "Artifact '$artifactName' not found in SHA256SUMS (available: $available). If a release was just published, Windows assets may still be building."
        }

        $downloadUrl = if (Test-LocalPath -Path $BaseDownloadUrl -PathType Container) {
            Join-Path $BaseDownloadUrl $artifactName
        } else {
            "$BaseDownloadUrl/$artifactName"
        }
        $downloadFile = Join-Path $TempDir $artifactName

        if (-not $Silent) {
            Write-Host "Downloading $downloadUrl..." -ForegroundColor Gray
        }
        Get-ReleaseFile -Url $downloadUrl -Destination $downloadFile

        # Step 3: Verify SHA-256 integrity
        if (-not $Silent) {
            Write-Host "[3/4] Verifying SHA-256 integrity..." -ForegroundColor Cyan
        }
        $computedHash = (Get-FileHash -LiteralPath $downloadFile -Algorithm SHA256).Hash.ToLowerInvariant()
        $expectedHash = $checksums[$artifactName].ToLowerInvariant()
        if ($computedHash -ne $expectedHash) {
            throw "SHA-256 mismatch for $artifactName!`nExpected: $expectedHash`nActual:   $computedHash"
        }
        if (-not $Silent) {
            Write-Host "[OK] SHA-256 verified: $computedHash" -ForegroundColor Green
        }

        # Step 4: Install
        if ($Portable) {
            if (-not $Silent) {
                Write-Host "[4/4] Extracting portable package..." -ForegroundColor Cyan
            }
            Stop-RunningMQ

            $targetDir = if ($InstallDir) { $InstallDir } else { $DefaultPortableDir }
            if (-not (Test-Path -LiteralPath $targetDir)) {
                New-Item -ItemType Directory -Path $targetDir -Force | Out-Null
            }

            # Extract into a staging directory first so both flat and nested portable ZIPs install cleanly
            $extractDir = Join-Path $TempDir "portable-extract"
            New-Item -ItemType Directory -Path $extractDir -Force | Out-Null
            Expand-Archive -LiteralPath $downloadFile -DestinationPath $extractDir -Force

            $payloadRoot = $extractDir
            if (-not (Test-Path -LiteralPath (Join-Path $extractDir "mq.exe"))) {
                $nestedDirs = @(Get-ChildItem -LiteralPath $extractDir -Directory)
                if ($nestedDirs.Count -eq 1 -and (Test-Path -LiteralPath (Join-Path $nestedDirs[0].FullName "mq.exe"))) {
                    $payloadRoot = $nestedDirs[0].FullName
                }
            }

            foreach ($item in @(Get-ChildItem -LiteralPath $payloadRoot -Force)) {
                $destPath = Join-Path $targetDir $item.Name
                if ($item.PSIsContainer -and ($item.Name -ieq "data") -and (Test-Path -LiteralPath $destPath)) {
                    # Preserve existing user portable data/ directory on upgrades and re-installs
                    continue
                }
                Copy-Item -LiteralPath $item.FullName -Destination $destPath -Recurse -Force
            }

            $flagPath = Join-Path $targetDir "portable.flag"
            if (-not (Test-Path -LiteralPath $flagPath)) {
                Set-Content -LiteralPath $flagPath -Value "# MQ Portable Mode" -Encoding UTF8
            }

            $installedExe = Join-Path $targetDir "mq.exe"
            if (-not (Test-Path -LiteralPath $installedExe)) {
                throw "Portable package did not contain mq.exe after extraction to $targetDir"
            }

            if (-not $Silent) {
                Write-Host "`n================================================================================" -ForegroundColor Green
                Write-Host " MQ LAUNCHER (PORTABLE) SUCCESSFULLY INSTALLED!" -ForegroundColor Green
                Write-Host "================================================================================" -ForegroundColor Green
                Write-Host " Directory:  $targetDir" -ForegroundColor Cyan
                Write-Host " Run:        $installedExe" -ForegroundColor Cyan
                Write-Host " Uninstall:  mq uninstall  (or: `$env:MQ_UNINSTALL=1; irm https://github.com/$Repo/releases/latest/download/install.ps1 | iex)" -ForegroundColor Gray
                Write-Host "================================================================================`n" -ForegroundColor Green
            }
        } else {
            if (-not $Silent) {
                Write-Host "[4/4] Executing silent installation..." -ForegroundColor Cyan
            }
            Stop-RunningMQ

            $setupArgs = @(
                "/VERYSILENT",
                "/SUPPRESSMSGBOXES",
                "/NORESTART",
                "/SP-"
            )
            if ($InstallDir) {
                $setupArgs += "/DIR=`"$InstallDir`""
            }
            if ($NoDesktopShortcut) {
                $setupArgs += "/MERGETASKS=!desktopicon"
            }

            $proc = Start-Process -FilePath $downloadFile -ArgumentList $setupArgs -Wait -PassThru -NoNewWindow
            if ($proc.ExitCode -ne 0) {
                throw "Installation failed with exit code $($proc.ExitCode)"
            }

            $installedDir = if ($InstallDir) { $InstallDir } else { $DefaultInstallDir }
            $installedExe = Join-Path $installedDir "mq.exe"
            Add-UserPathEntry -DirPath $installedDir

            if (-not $Silent) {
                Write-Host "`n================================================================================" -ForegroundColor Green
                Write-Host " MQ LAUNCHER SUCCESSFULLY INSTALLED!" -ForegroundColor Green
                Write-Host "================================================================================" -ForegroundColor Green
                Write-Host " Executable: $installedExe" -ForegroundColor Cyan
                Write-Host " Start Menu: MQ" -ForegroundColor Cyan
                Write-Host " CLI:        mq --version (added to User PATH)" -ForegroundColor Cyan
                Write-Host " User Data:  $UserDataDir (Preserved on uninstalls and updates)" -ForegroundColor Gray
                Write-Host " Uninstall:  mq uninstall  (or: `$env:MQ_UNINSTALL=1; irm https://github.com/$Repo/releases/latest/download/install.ps1 | iex)" -ForegroundColor Gray
                Write-Host "================================================================================`n" -ForegroundColor Green
            }
        }
    }
    finally {
        if (Test-Path -LiteralPath $TempDir) {
            Remove-Item -LiteralPath $TempDir -Recurse -Force -ErrorAction SilentlyContinue
        }
    }
}

$scriptArgs = @($args)
if ($scriptArgs.Count -gt 0) {
    $boundParams = @{}
    $switchParams = @("Portable", "Silent", "Uninstall", "Purge", "NoDesktopShortcut", "Force", "Help")
    $valueParams = @("Version", "InstallDir", "BaseUrl")
    for ($i = 0; $i -lt $scriptArgs.Count; $i++) {
        $argToken = [string]$scriptArgs[$i]
        if ($argToken -match '^-([A-Za-z]+)(?::(.*))?$') {
            $argName = $matches[1]
            $inlineVal = if ($matches.ContainsKey(2)) { $matches[2] } else { $null }
            $matchedSwitch = $switchParams | Where-Object { $_ -ieq $argName } | Select-Object -First 1
            if ($matchedSwitch) {
                if ($null -ne $inlineVal -and $inlineVal -ne "") {
                    $boundParams[$matchedSwitch] = ($inlineVal -match '^(?i:1|true|\$true)$')
                } elseif ($argToken.EndsWith(":") -and ($i + 1) -lt $scriptArgs.Count -and ($scriptArgs[$i + 1] -is [bool])) {
                    $i++
                    $boundParams[$matchedSwitch] = [bool]$scriptArgs[$i]
                } else {
                    $boundParams[$matchedSwitch] = $true
                }
                continue
            }
            $matchedVal = $valueParams | Where-Object { $_ -ieq $argName } | Select-Object -First 1
            if ($matchedVal) {
                if ($null -ne $inlineVal -and $inlineVal -ne "") {
                    $boundParams[$matchedVal] = $inlineVal
                } elseif (($i + 1) -lt $scriptArgs.Count) {
                    $i++
                    $boundParams[$matchedVal] = [string]$scriptArgs[$i]
                }
                continue
            }
            throw "Unknown parameter: -$argName"
        } elseif (-not $boundParams.ContainsKey("Version")) {
            $boundParams["Version"] = $argToken
        } else {
            throw "Unexpected positional argument: $argToken"
        }
    }
    Invoke-MqInstaller @boundParams
} else {
    Invoke-MqInstaller
}
