$ErrorActionPreference = 'Stop'

$root = Join-Path $env:TEMP ("wt-media-agent-stop-" + [guid]::NewGuid().ToString('N'))
$first = Join-Path $root 'installed app'
$second = Join-Path $root 'other app'
$script = Join-Path $PSScriptRoot '..\src-tauri\windows\stop-installed-agent.ps1'

try {
    New-Item -ItemType Directory -Path $first, $second -Force | Out-Null
    $firstExe = Join-Path $first 'wt-media-agent.exe'
    $secondExe = Join-Path $second 'wt-media-agent.exe'
    Copy-Item "$env:SystemRoot\System32\cmd.exe" $firstExe
    Copy-Item "$env:SystemRoot\System32\cmd.exe" $secondExe

    $target = Start-Process -FilePath $firstExe -ArgumentList '/c ping 127.0.0.1 -n 60 >nul' -WindowStyle Hidden -PassThru
    $unrelated = Start-Process -FilePath $secondExe -ArgumentList '/c ping 127.0.0.1 -n 60 >nul' -WindowStyle Hidden -PassThru
    Start-Sleep -Seconds 1
    if ($target.HasExited -or $unrelated.HasExited) {
        throw 'Fixture processes exited before the lifecycle check.'
    }

    & "$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File $script $firstExe
    if ($LASTEXITCODE -ne 0) {
        throw "Agent stopper failed with exit code $LASTEXITCODE."
    }
    $target.Refresh()
    $unrelated.Refresh()
    if (-not $target.HasExited) {
        throw 'The Agent at the installed path is still running.'
    }
    if ($unrelated.HasExited) {
        throw 'An Agent at a different path was stopped.'
    }

    Write-Host 'Installed Agent stopped; same-name Agent at another path survived.'
} finally {
    if ($target -and -not $target.HasExited) { Stop-Process -Id $target.Id -Force }
    if ($unrelated -and -not $unrelated.HasExited) { Stop-Process -Id $unrelated.Id -Force }
    Remove-Item $root -Recurse -Force -ErrorAction SilentlyContinue
}
