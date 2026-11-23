<#
.SYNOPSIS
Build CqShell Linux release in WSL and deploy atomically to a remote Linux server.

.EXAMPLE
.\scripts\deploy.ps1
Fast incremental Linux release build + deploy using `.deploy.local.json`.

.EXAMPLE
.\scripts\deploy.ps1 -Fmt -Check
Auto-format, run full Linux fmt/clippy/test/test_install.py, then build and deploy.

.EXAMPLE
.\scripts\deploy.ps1 -Ssh root@1.2.3.4
Build and deploy to any SSH server via standard `ssh`.
#>
[CmdletBinding()]
param(
    [switch]$Fmt,
    [switch]$Check,
    [switch]$SkipBuild,
    [string]$Ssh,
    [int]$Port,
    [string]$Identity,
    [string]$MottySession,
    [string]$WslDistro,
    [string]$RemotePath
)

$ErrorActionPreference = 'Stop'
$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$pyArgs = @((Join-Path $scriptDir 'deploy.py'))

if ($Fmt) { $pyArgs += '--fmt' }
if ($Check) { $pyArgs += '--check' }
if ($SkipBuild) { $pyArgs += '--skip-build' }
if ($Ssh) { $pyArgs += @('--ssh', $Ssh) }
if ($Port) { $pyArgs += @('--port', $Port.ToString()) }
if ($Identity) { $pyArgs += @('--identity', $Identity) }
if ($MottySession) { $pyArgs += @('--motty-session', $MottySession) }
if ($WslDistro) { $pyArgs += @('--wsl-distro', $WslDistro) }
if ($RemotePath) { $pyArgs += @('--remote-path', $RemotePath) }

& python @pyArgs
exit $LASTEXITCODE
