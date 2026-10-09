param(
    [Parameter(Mandatory = $true)]
    [string] $ExecutablePath
)

$ErrorActionPreference = 'Stop'

try {
    $expectedPath = [System.IO.Path]::GetFullPath($ExecutablePath)
    $findInstalledAgent = {
        @(Get-CimInstance Win32_Process -Filter "Name = 'wt-media-agent.exe'" |
            Where-Object {
                $_.ExecutablePath -and
                [string]::Equals(
                    [System.IO.Path]::GetFullPath($_.ExecutablePath),
                    $expectedPath,
                    [System.StringComparison]::OrdinalIgnoreCase
                )
            })
    }

    foreach ($agent in (& $findInstalledAgent)) {
        # A PyInstaller parent and child can exit together. A PID disappearing
        # between the snapshot and this call is success, not an install error.
        Stop-Process -Id $agent.ProcessId -Force -ErrorAction SilentlyContinue
    }

    # Wait for the executable handle to close before NSIS overwrites/deletes it.
    for ($attempt = 0; $attempt -lt 50; $attempt++) {
        if ((& $findInstalledAgent).Count -eq 0) {
            exit 0
        }
        Start-Sleep -Milliseconds 200
    }

    throw "Installed Agent is still running: $expectedPath"
} catch {
    [Console]::Error.WriteLine("Could not stop the installed Agent: $($_.Exception.Message)")
    exit 1
}
