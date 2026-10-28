<#
.SYNOPSIS
  Registers the Ferry daemon (`ferry daemon`) in Task Scheduler to start at logon.

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File install-autostart.ps1
  powershell -ExecutionPolicy Bypass -File install-autostart.ps1 -Uninstall

.NOTES
  Without a console the daemon only accepts files from Auto-Accept devices.
  The GUI app has its own "Start at login" switch in Settings.
#>
param(
    [string]$FerryPath = (Join-Path $env:USERPROFILE ".cargo\bin\ferry.exe"),
    [switch]$Uninstall
)

$ErrorActionPreference = "Stop"
$TaskName = "Ferry Daemon"

if ($Uninstall) {
    Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false -ErrorAction SilentlyContinue
    Write-Host "Removed task '$TaskName'."
    exit 0
}

if (-not (Test-Path $FerryPath)) {
    throw "$FerryPath not found. Pass the path: -FerryPath C:\path\to\ferry.exe"
}

$action = New-ScheduledTaskAction -Execute $FerryPath -Argument "daemon"
$trigger = New-ScheduledTaskTrigger -AtLogOn -User $env:USERNAME
$settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries `
    -ExecutionTimeLimit ([TimeSpan]::Zero) -RestartCount 3 -RestartInterval (New-TimeSpan -Minutes 1) -Hidden
$principal = New-ScheduledTaskPrincipal -UserId $env:USERNAME -LogonType Interactive -RunLevel Limited

Register-ScheduledTask -TaskName $TaskName -Action $action -Trigger $trigger -Settings $settings `
    -Principal $principal -Description "Ferry: background P2P file receiving" -Force | Out-Null
Start-ScheduledTask -TaskName $TaskName
Write-Host "Registered and started '$TaskName'."
