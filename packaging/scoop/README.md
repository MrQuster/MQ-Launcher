# Scoop Manifest for MQ Launcher

This directory contains the Scoop manifest `mq.json` for installing MQ Launcher via [Scoop](https://scoop.sh).

## User Installation

Users can install MQ Launcher using Scoop by adding the repository as a bucket or installing directly from the URL:

```powershell
# Option 1: Direct install from URL
scoop install https://raw.githubusercontent.com/MrQuster/mq/main/packaging/scoop/mq.json

# Option 2: Add custom bucket
scoop bucket add mq https://github.com/MrQuster/mq
scoop install mq
```

## Features

- **Isolated & Portable**: Game instances and configurations are saved in Scoop's `persist` directory (`data/`) and preserved across updates. Nothing is written to the Windows Registry.
- **Auto-Updates**: Automatically tracks GitHub Releases for new versions via Scoop's `checkver` and `autoupdate`.
- **Command-line & GUI**: Adds `mq` to the system `PATH` and registers a Windows Start Menu shortcut (`MQ`).

## Portable Archive Layout (`mq-windows-x86_64-portable.zip`)

The portable ZIP archive uses a flat root layout (compatible with Scoop, `install.ps1 -Portable`, and Windows Explorer "Extract All..."):

```text
mq-windows-x86_64-portable.zip
├── mq.exe                    # Statically linked Windows x86_64 binary
├── portable.flag             # Marker file enabling portable mode (data/ next to mq.exe)
├── mq.ico                    # Application icon
├── run.bat                   # Quick-launch helper script
├── README.txt                # Quick-start and portable mode instructions
├── THIRD_PARTY_LICENSES.md   # Third-party license notices
└── data/                     # Empty portable data root (preserved on upgrades)
```

When `portable.flag` is present next to `mq.exe`, MQ stores all launcher data, instances, configs, and caches under `<exe_dir>\data\` (`data/config`, `data/cache`, `data/instances`, etc.) and writes nothing to the Windows Registry.

### Building the Portable Archive Locally (PowerShell)

```powershell
pwsh -NoProfile -File packaging\windows\package-portable.ps1
```
