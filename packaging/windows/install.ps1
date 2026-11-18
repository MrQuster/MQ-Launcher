# ==============================================================================
#  MQ Launcher -- 1-Command Universal Windows Installer
#  Usage:
#    irm https://github.com/MrQuster/mq/releases/latest/download/install.ps1 | iex
#  Advanced:
#    & ([scriptblock]::Create((irm https://github.com/MrQuster/mq/releases/latest/download/install.ps1))) -Portable
#    & ([scriptblock]::Create((irm https://github.com/MrQuster/mq/releases/latest/download/install.ps1))) -Uninstall
#    & ([scriptblock]::Create((irm https://github.com/MrQuster/mq/releases/latest/download/install.ps1))) -Version 2.6.9 -Silent
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
    if (-not $Portable -and ($envPortable -eq "1" -or $envPortable -eq "true")) { $Portable = $true }

    $envSilent = [Environment]::GetEnvironmentVariable("MQ_SILENT")
    if (-not $Silent -and ($envSilent -eq "1" -or $envSilent -eq "true")) { $Silent = $true }

    $envUninstall = [Environment]::GetEnvironmentVariable("MQ_UNINSTALL")
    if (-not $Uninstall -and ($envUninstall -eq "1" -or $envUninstall -eq "true")) { $Uninstall = $true }

    $envPurge = [Environment]::GetEnvironmentVariable("MQ_PURGE")
    if (-not $Purge -and ($envPurge -eq "1" -or $envPurge -eq "true")) { $Purge = $true }

    $envNoDesktop = [Environment]::GetEnvironmentVariable("MQ_NO_DESKTOP_SHORTCUT")
    if (-not $NoDesktopShortcut -and ($envNoDesktop -eq "1" -or $envNoDesktop -eq "true")) { $NoDesktopShortcut = $true }

    $envForce = [Environment]::GetEnvironmentVariable("MQ_FORCE")
    if (-not $Force -and ($envForce -eq "1" -or $envForce -eq "true")) { $Force = $true }

    if (-not $InstallDir) { $InstallDir = [Environment]::GetEnvironmentVariable("MQ_INSTALL_DIR") }
    if (-not $BaseUrl) { $BaseUrl = [Environment]::GetEnvironmentVariable("MQ_BASE_URL") }

    if ($Help) {
        Write-Host @"
MQ Launcher -- Windows Installer
Usage:
  install.ps1 [-Version <tag>] [-Portable] [-Silent] [-Uninstall] [-Purge] [-InstallDir <path>] [-BaseUrl <url>] [-NoDesktopShortcut] [-Force]

Parameters:
  -Version <tag>       Specify version to install (e.g. '2.6.9' or 'latest'). Default: latest
  -Portable            Install standalone portable package into %LOCALAPPDATA%\MQ-Portable
  -Silent              Run unattended installation (/VERYSILENT /SUPPRESSMSGBOXES /NORESTART)
  -Uninstall           Silently uninstall MQ Launcher (preserves %LOCALAPPDATA%\MQ user data)
  -Purge               With -Uninstall, also remove all user data, instances, mods and settings
  -InstallDir <path>   Custom installation target directory
  -BaseUrl <url>       Override release download base URL (for local/CI testing)
  -NoDesktopShortcut   Do not create a shortcut on Desktop
  -Force               Ignore active running instances
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

    # 2. Enforce modern secure TLS protocols (TLS 1.2 & TLS 1.3)
    try {
        [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
        if ([Enum]::IsDefined([Net.SecurityProtocolType], "Tls13")) {
            [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls13
        }
    } catch {
        [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor 3072 -bor 12288
    }

    # Repository & Minisign configuration
    $envRepo = [Environment]::GetEnvironmentVariable("MQ_REPO")
    $Repo = if ($envRepo) { $envRepo } else { "MrQuster/mq" }
    $MinisignPubKey = "RWRlxrUOJgeLq7AIndJRG9ZhsdT5QiHy9usykXvxt1C49a1h/km3h9lt"
    $AppName = "MQ Launcher"
    $DefaultInstallDir = Join-Path $env:LOCALAPPDATA "Programs\MQ"
    $DefaultPortableDir = Join-Path $env:LOCALAPPDATA "MQ-Portable"
    $UserDataDir = Join-Path $env:LOCALAPPDATA "MQ"

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

    # Helper function to close running instances of mq.exe only
    function Stop-RunningMQ {
        $processes = Get-Process -Name "mq" -ErrorAction SilentlyContinue
        if ($processes) {
            if (-not $Silent) {
                Write-Host "Closing running instances of $AppName..." -ForegroundColor Yellow
            }
            foreach ($proc in $processes) {
                try {
                    Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
                } catch {}
            }
            Start-Sleep -Milliseconds 300
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
        try {
            Invoke-WebRequest -Uri $Url -OutFile $Destination -UseBasicParsing
        } catch {
            $wc = New-Object System.Net.WebClient
            $wc.DownloadFile($Url, $Destination)
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

        # Prefer default per-user uninstaller (%LOCALAPPDATA%\Programs\MQ\unins000.exe)
        $defaultUnins = Join-Path $DefaultInstallDir "unins000.exe"
        if (Test-Path $defaultUnins) {
            if (-not $Silent) {
                Write-Host "Running uninstaller: $defaultUnins" -ForegroundColor Yellow
            }
            $proc = Start-Process -FilePath $defaultUnins -ArgumentList "/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART" -Wait -PassThru -NoNewWindow
            if ($proc.ExitCode -ne 0) {
                throw "Uninstaller failed with exit code $($proc.ExitCode)"
            }
            $uninstalled = $true
        }

        # Check Windows Registry for Inno Setup uninstaller if not found at default path
        if (-not $uninstalled) {
            $registryKeys = @(
                "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*",
                "HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*"
            )
            foreach ($regKey in $registryKeys) {
                $matchesFound = @(Get-ItemProperty -Path $regKey -ErrorAction SilentlyContinue |
                    Where-Object { ($_.PSObject.Properties['DisplayName']) -and ($_.DisplayName -like "*MQ*") -and ($_.PSObject.Properties['UninstallString']) -and $_.UninstallString })

                if ($matchesFound.Count -gt 0) {
                    $rawCmd = $matchesFound[0].UninstallString.Trim('"')
                    if (Test-Path $rawCmd) {
                        if (-not $Silent) {
                            Write-Host "Running uninstaller: $rawCmd" -ForegroundColor Yellow
                        }
                        $proc = Start-Process -FilePath $rawCmd -ArgumentList "/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART" -Wait -PassThru -NoNewWindow
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
            if (Test-Path $dirPath) {
                if (-not $Silent) {
                    Write-Host "Removing installation directory $dirPath..." -ForegroundColor Yellow
                }
                Remove-Item -Path $dirPath -Recurse -Force -ErrorAction SilentlyContinue
                $uninstalled = $true
            }
        }

        # Clean up leftover shortcuts if any
        $shortcuts = @(
            (Join-Path ([Environment]::GetFolderPath("Programs")) "MQ.lnk"),
            (Join-Path ([Environment]::GetFolderPath("Programs")) "MQ Launcher.lnk"),
            (Join-Path ([Environment]::GetFolderPath("Desktop")) "MQ.lnk"),
            (Join-Path ([Environment]::GetFolderPath("Desktop")) "MQ Launcher.lnk")
        )
        foreach ($sc in $shortcuts) {
            if (Test-Path $sc) {
                Remove-Item -Path $sc -Force -ErrorAction SilentlyContinue
            }
        }

        # Clean MQ install directories from User PATH
        $currentUserPath = [Environment]::GetEnvironmentVariable("Path", "User")
        if ($currentUserPath) {
            $cleanedParts = @($currentUserPath -split ';' | Where-Object {
                $item = $_.Trim()
                $item -and ($installCandidates -notcontains $item)
            })
            $newUserPath = ($cleanedParts -join ';')
            if ($newUserPath -ne $currentUserPath) {
                [Environment]::SetEnvironmentVariable("Path", $newUserPath, "User")
            }
        }

        # Purge user data if -Purge / MQ_PURGE=1
        if ($Purge) {
            $roamingDataDir = Join-Path $env:APPDATA "MQ"
            foreach ($d in @($UserDataDir, $roamingDataDir)) {
                if ($d -and (Test-Path $d)) {
                    if (-not $Silent) {
                        Write-Host "Purging user data directory $d..." -ForegroundColor Yellow
                    }
                    Remove-Item -Path $d -Recurse -Force -ErrorAction SilentlyContinue
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
        Get-Content -Path $SumsFile | ForEach-Object {
            $line = $_.Trim()
            if ($line -and -not $line.StartsWith("#")) {
                if ($line -match '^([a-fA-F0-9]{64})\s+\*?(.+)$') {
                    $hash = $matches[1].ToLower()
                    $fname = [System.IO.Path]::GetFileName($matches[2].Trim())
                    $checksums[$fname] = $hash
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
            foreach ($k in $checksums.Keys) {
                if ($k -match '(?i)^mq-.*portable.*\.zip$') {
                    $artifactName = $k
                    break
                }
            }
        } else {
            $verClean = $Version.TrimStart('v')
            $artifactName = if ($Version -eq "latest") { "MQ-Setup.exe" } else { "MQ-Setup-$verClean-x64.exe" }
            foreach ($k in $checksums.Keys) {
                if ($k -match '(?i)^MQ-Setup.*-x64\.exe$') {
                    $artifactName = $k
                    break
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
            throw "Artifact '$artifactName' not found in SHA256SUMS."
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
        $computedHash = (Get-FileHash -Path $downloadFile -Algorithm SHA256).Hash.ToLower()
        $expectedHash = $checksums[$artifactName]
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
            if (-not (Test-Path $targetDir)) {
                New-Item -ItemType Directory -Path $targetDir -Force | Out-Null
            }

            Expand-Archive -Path $downloadFile -DestinationPath $targetDir -Force

            $flagPath = Join-Path $targetDir "portable.flag"
            if (-not (Test-Path $flagPath)) {
                Set-Content -Path $flagPath -Value "# MQ Portable Mode" -Encoding UTF8
            }

            if (-not $Silent) {
                Write-Host "`n================================================================================" -ForegroundColor Green
                Write-Host " MQ LAUNCHER (PORTABLE) SUCCESSFULLY INSTALLED!" -ForegroundColor Green
                Write-Host "================================================================================" -ForegroundColor Green
                Write-Host " Directory:  $targetDir" -ForegroundColor Cyan
                Write-Host " Run:        $(Join-Path $targetDir 'mq.exe')" -ForegroundColor Cyan
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
                $setupArgs += "/DIR=$InstallDir"
            }
            if ($NoDesktopShortcut) {
                $setupArgs += "/TASKS=!desktopicon"
            }

            $proc = Start-Process -FilePath $downloadFile -ArgumentList $setupArgs -Wait -PassThru -NoNewWindow
            if ($proc.ExitCode -ne 0) {
                throw "Installation failed with exit code $($proc.ExitCode)"
            }

            $installedExe = if ($InstallDir) { Join-Path $InstallDir "mq.exe" } else { Join-Path $DefaultInstallDir "mq.exe" }

            if (-not $Silent) {
                Write-Host "`n================================================================================" -ForegroundColor Green
                Write-Host " MQ LAUNCHER SUCCESSFULLY INSTALLED!" -ForegroundColor Green
                Write-Host "================================================================================" -ForegroundColor Green
                Write-Host " Executable: $installedExe" -ForegroundColor Cyan
                Write-Host " Start Menu: MQ" -ForegroundColor Cyan
                Write-Host " User Data:  $UserDataDir (Preserved on uninstalls and updates)" -ForegroundColor Gray
                Write-Host " Uninstall:  mq uninstall  (or: `$env:MQ_UNINSTALL=1; irm https://github.com/$Repo/releases/latest/download/install.ps1 | iex)" -ForegroundColor Gray
                Write-Host "================================================================================`n" -ForegroundColor Green
            }
        }
    }
    finally {
        if (Test-Path $TempDir) {
            Remove-Item -Path $TempDir -Recurse -Force -ErrorAction SilentlyContinue
        }
    }
}

if ($args -and @($args).Count -gt 0) {
    $boundParams = @{}
    $switchParams = @("Portable", "Silent", "Uninstall", "Purge", "NoDesktopShortcut", "Force", "Help")
    $valueParams = @("Version", "InstallDir", "BaseUrl")
    for ($i = 0; $i -lt $args.Count; $i++) {
        $argToken = [string]$args[$i]
        if ($argToken -match '^-([A-Za-z]+)(?::(.*))?$') {
            $argName = $matches[1]
            $inlineVal = if ($matches.ContainsKey(2)) { $matches[2] } else { $null }
            $matchedSwitch = $switchParams | Where-Object { $_ -ieq $argName } | Select-Object -First 1
            if ($matchedSwitch) {
                if ($null -ne $inlineVal -and $inlineVal -ne "") {
                    $boundParams[$matchedSwitch] = ($inlineVal -match '^(?i:1|true|\$true)$')
                } elseif ($argToken.EndsWith(":") -and ($i + 1) -lt $args.Count -and ($args[$i + 1] -is [bool])) {
                    $i++
                    $boundParams[$matchedSwitch] = [bool]$args[$i]
                } else {
                    $boundParams[$matchedSwitch] = $true
                }
                continue
            }
            $matchedVal = $valueParams | Where-Object { $_ -ieq $argName } | Select-Object -First 1
            if ($matchedVal) {
                if ($null -ne $inlineVal -and $inlineVal -ne "") {
                    $boundParams[$matchedVal] = $inlineVal
                } elseif (($i + 1) -lt $args.Count) {
                    $i++
                    $boundParams[$matchedVal] = [string]$args[$i]
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
