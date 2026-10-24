<#
.SYNOPSIS
  Registers the RQS daemon (`rqs daemon`) in Task Scheduler to start at logon.

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File install-autostart.ps1
  powershell -ExecutionPolicy Bypass -File install-autostart.ps1 -Uninstall

.NOTES
  Without a console the daemon only accepts files from Auto-Accept devices.
  The GUI app has its own "Start at login" switch in Settings.
#>
param(
    [string]$RqsPath = (Join-Path $env:USERPROFILE ".cargo\bin\rqs.exe"),
    [switch]$Uninstall
)

$ErrorActionPreference = "Stop"
$TaskName = "RQS P2P Quick Share Daemon"

if ($Uninstall) {
    Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false -ErrorAction SilentlyContinue
    Write-Host "Removed task '$TaskName'."
    exit 0
}

if (-not (Test-Path $RqsPath)) {
    throw "$RqsPath not found. Pass the path: -RqsPath C:\path\to\rqs.exe"
}

$action = New-ScheduledTaskAction -Execute $RqsPath -Argument "daemon"
$trigger = New-ScheduledTaskTrigger -AtLogOn -User $env:USERNAME
$settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries `
    -ExecutionTimeLimit ([TimeSpan]::Zero) -RestartCount 3 -RestartInterval (New-TimeSpan -Minutes 1) -Hidden
$principal = New-ScheduledTaskPrincipal -UserId $env:USERNAME -LogonType Interactive -RunLevel Limited

Register-ScheduledTask -TaskName $TaskName -Action $action -Trigger $trigger -Settings $settings `
    -Principal $principal -Description "RQS P2P background file receiving" -Force | Out-Null
Start-ScheduledTask -TaskName $TaskName
Write-Host "Registered and started '$TaskName'."
