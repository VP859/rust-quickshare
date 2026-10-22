<#
.SYNOPSIS
  Rejestruje demona RQS (`rqs daemon`) w Harmonogramie zadań – start po zalogowaniu.

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File install-autostart.ps1
  powershell -ExecutionPolicy Bypass -File install-autostart.ps1 -Uninstall

.NOTES
  Bez konsoli demon przyjmuje pliki wyłącznie od urządzeń z Auto-Accept.
  Aplikacja GUI ma własny przełącznik „Uruchamiaj przy starcie systemu” w Ustawieniach.
#>
param(
    [string]$RqsPath = (Join-Path $env:USERPROFILE ".cargo\bin\rqs.exe"),
    [switch]$Uninstall
)

$ErrorActionPreference = "Stop"
$TaskName = "RQS P2P Quick Share Daemon"

if ($Uninstall) {
    Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false -ErrorAction SilentlyContinue
    Write-Host "Usunięto zadanie '$TaskName'."
    exit 0
}

if (-not (Test-Path $RqsPath)) {
    throw "Nie znaleziono $RqsPath. Podaj ścieżkę: -RqsPath C:\sciezka\rqs.exe"
}

$action = New-ScheduledTaskAction -Execute $RqsPath -Argument "daemon"
$trigger = New-ScheduledTaskTrigger -AtLogOn -User $env:USERNAME
$settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries `
    -ExecutionTimeLimit ([TimeSpan]::Zero) -RestartCount 3 -RestartInterval (New-TimeSpan -Minutes 1) -Hidden
$principal = New-ScheduledTaskPrincipal -UserId $env:USERNAME -LogonType Interactive -RunLevel Limited

Register-ScheduledTask -TaskName $TaskName -Action $action -Trigger $trigger -Settings $settings `
    -Principal $principal -Description "Odbiór plików RQS P2P w tle" -Force | Out-Null
Start-ScheduledTask -TaskName $TaskName
Write-Host "Zarejestrowano i uruchomiono '$TaskName'."
