# Register context menu for all file types on Windows
# Run as Administrator: powershell -ExecutionPolicy Bypass -File register-context-menu-windows.ps1

$regPath = "Registry::HKEY_CLASSES_ROOT\*\shell\GuvercinSend"
$cmdPath = "$regPath\command"
$appPath = "C:\Program Files\guvercin\guvercin.exe"

try {
    # Check if app is installed
    if (-not (Test-Path $appPath)) {
        Write-Host "Error: Guvercin not found at $appPath"
        Write-Host "Please make sure Guvercin is installed before registering context menu."
        exit 1
    }

    # Create registry entries
    New-Item -Path $regPath -Force | Out-Null
    New-ItemProperty -Path $regPath -Name "(Default)" -Value "Guvercin ile Gönder" -PropertyType String -Force | Out-Null
    New-ItemProperty -Path $regPath -Name "Icon" -Value $appPath -PropertyType String -Force | Out-Null

    # Create command entry
    New-Item -Path $cmdPath -Force | Out-Null
    New-ItemProperty -Path $cmdPath -Name "(Default)" -Value "`"$appPath`" --file-attachment `"%1`"" -PropertyType String -Force | Out-Null

    Write-Host "✓ Context menu registered successfully!"
    Write-Host "Right-click any file and select 'Guvercin ile Gönder' to send it via Guvercin."
} catch {
    Write-Host "Error: $_"
    exit 1
}
