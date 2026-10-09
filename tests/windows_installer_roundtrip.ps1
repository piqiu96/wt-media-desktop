param(
    [Parameter(Mandatory = $true)]
    [string] $Installer
)

$ErrorActionPreference = 'Stop'
$Installer = (Resolve-Path $Installer).Path
$installDir = Join-Path $env:LOCALAPPDATA '起飞'
$sidecar = Join-Path $installDir 'wt-media-agent.exe'
$uninstaller = Join-Path $installDir 'uninstall.exe'
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
    if (Test-Path $installDir) {
        throw "Installer smoke test requires a clean runner: $installDir already exists."
    }

    Invoke-Setup $Installer
    if (-not (Test-Path $sidecar)) { throw 'First install did not place the Agent executable.' }
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
