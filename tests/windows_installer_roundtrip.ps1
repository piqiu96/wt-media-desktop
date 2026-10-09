param(
    [Parameter(Mandatory = $true)]
    [string] $Installer
)

$ErrorActionPreference = 'Stop'
$Installer = (Resolve-Path $Installer).Path
$productName = ([System.IO.Path]::GetFileName($Installer) -split '_', 2)[0]
if (-not $productName) { throw "Could not derive product name from $Installer." }
$uninstallKey = Join-Path 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall' $productName
$expectedInstallDir = Join-Path $env:LOCALAPPDATA $productName
$installDir = $null
$sidecar = $null
$uninstaller = $null
$sentinel = Join-Path $env:LOCALAPPDATA 'WTMedia\Desktop\data\ci-reinstall-preserve.txt'
$running = $null

function Invoke-Setup([string] $path) {
    $result = Start-Process -FilePath $path -ArgumentList '/S' -Wait -PassThru
    if ($result.ExitCode -ne 0) {
        throw "$path exited with $($result.ExitCode)."
    }
}

function Start-Agent {
    $env:WT_MEDIA_AGENT_RUNTIME_TOKEN = [guid]::NewGuid().ToString('N')
    $env:WT_MEDIA_LOCAL_API_PORT = '0'
    $env:WT_MEDIA_AGENT_RUN_RUNNER = 'false'
    $script:running = Start-Process -FilePath $sidecar -WindowStyle Hidden -PassThru
    Start-Sleep -Seconds 3
    $script:running.Refresh()
    if ($script:running.HasExited) {
        throw "Packaged Agent exited before setup could stop it: $($script:running.ExitCode)."
    }
}

try {
    if (Test-Path -LiteralPath $expectedInstallDir) {
        throw "Installer smoke test requires a clean runner: $expectedInstallDir already exists."
    }

    Invoke-Setup $Installer
    $registration = Get-ItemProperty -LiteralPath $uninstallKey -ErrorAction Stop
    $installDir = $registration.InstallLocation.Trim('"')
    $sidecar = Join-Path $installDir 'wt-media-agent.exe'
    $uninstaller = Join-Path $installDir 'uninstall.exe'
    Write-Host "Registered install directory: $installDir"
    if (-not (Test-Path -LiteralPath $sidecar)) {
        throw "First install did not place the Agent executable at $sidecar."
    }
    New-Item -ItemType Directory -Path (Split-Path $sentinel) -Force | Out-Null
    Set-Content -Path $sentinel -Value 'keep on reinstall'

    Start-Agent
    Invoke-Setup $Installer
    $running.Refresh()
    if (-not $running.HasExited) { throw 'Reinstall left the previous Agent running.' }
    if (-not (Test-Path $sidecar)) { throw 'Reinstall lost the Agent executable.' }
    if (-not (Test-Path $sentinel)) { throw 'Reinstall removed existing application data.' }

    Start-Agent
    Invoke-Setup $uninstaller
    $running.Refresh()
    if (-not $running.HasExited) { throw 'Uninstall left the Agent running.' }
    if (Test-Path $sidecar) { throw 'Uninstall left the Agent executable behind.' }

    Write-Host 'Install, reinstall, and uninstall stopped the installed Agent at the correct points.'
} finally {
    if ($running) {
        $running.Refresh()
        if (-not $running.HasExited) { Stop-Process -Id $running.Id -Force }
    }
    Remove-Item $sentinel -Force -ErrorAction SilentlyContinue
}
