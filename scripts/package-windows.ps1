param(
    [string]$Version = "0.8.0-alpha",
    [string]$Configuration = "release"
)

$ErrorActionPreference = "Stop"
$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
$buildDirectory = Join-Path $repoRoot "target\$Configuration"
$outputDirectory = Join-Path $repoRoot "dist"
$packageDirectory = Join-Path $outputDirectory "package-files"

if (-not (Test-Path -LiteralPath (Join-Path $buildDirectory "swagri-agent.exe"))) {
    throw "swagri-agent.exe was not found. Run cargo build --release first."
}
if (-not (Test-Path -LiteralPath (Join-Path $buildDirectory "swagri-debugger.exe"))) {
    throw "swagri-debugger.exe was not found. Run cargo build --release first."
}
if (-not (Test-Path -LiteralPath (Join-Path $buildDirectory "swagri-updater.exe"))) {
    throw "swagri-updater.exe was not found. Run cargo build --release first."
}

$resolvedOutput = [System.IO.Path]::GetFullPath($outputDirectory)
if (-not $resolvedOutput.StartsWith($repoRoot, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Refusing to package outside the repository."
}
if (Test-Path -LiteralPath $outputDirectory) {
    Remove-Item -LiteralPath $outputDirectory -Recurse -Force
}
New-Item -ItemType Directory -Path $packageDirectory -Force | Out-Null

$readme = @"
Swagri $Version experimental build

Debugger package:
  Run swagri-debugger.exe. It starts the bundled agent, shows host metrics,
  peer capacity scores, offers smart CPU and matrix tests, can pause this
  computer's Swagri contribution, and keeps a live task activity/history panel.

Agent package:
  Run swagri-agent.exe --name <device-name>
  Type help for commands. The persistent identity is stored under LocalAppData.
  Resource sampling defaults to 5 seconds; CPU calibration is cached after one run.

Updates: both packages include swagri-updater.exe. Trust a specific Peer ID
before receiving signed P2P updates. Debugger packages can share both Agent and
GUI updates; headless packages share Agent only. Use trusted test networks.
Documentation: https://github.com/vitalii87/swagri
"@
Set-Content -LiteralPath (Join-Path $packageDirectory "README.txt") -Value $readme -Encoding utf8
$binaryVersion = $Version.Split('-')[0]
Set-Content -LiteralPath (Join-Path $packageDirectory "swagri-debugger.version") -Value $binaryVersion -Encoding ascii

$agentPortable = Join-Path $outputDirectory "agent-portable"
$debuggerPortable = Join-Path $outputDirectory "debugger-portable"
New-Item -ItemType Directory -Path $agentPortable, $debuggerPortable -Force | Out-Null
Copy-Item -LiteralPath (Join-Path $buildDirectory "swagri-agent.exe") -Destination $agentPortable
Copy-Item -LiteralPath (Join-Path $buildDirectory "swagri-updater.exe") -Destination $agentPortable
Copy-Item -LiteralPath (Join-Path $buildDirectory "swagri-agent.exe") -Destination $debuggerPortable
Copy-Item -LiteralPath (Join-Path $buildDirectory "swagri-debugger.exe") -Destination $debuggerPortable
Copy-Item -LiteralPath (Join-Path $buildDirectory "swagri-updater.exe") -Destination $debuggerPortable
Copy-Item -LiteralPath (Join-Path $packageDirectory "README.txt") -Destination $agentPortable
Copy-Item -LiteralPath (Join-Path $packageDirectory "README.txt") -Destination $debuggerPortable
Copy-Item -LiteralPath (Join-Path $packageDirectory "swagri-debugger.version") -Destination $debuggerPortable

Compress-Archive -Path (Join-Path $agentPortable "*") -DestinationPath (Join-Path $outputDirectory "Swagri-Agent-Portable-x64.zip")
Compress-Archive -Path (Join-Path $debuggerPortable "*") -DestinationPath (Join-Path $outputDirectory "Swagri-Debugger-Portable-x64.zip")

$makensis = Get-Command "makensis.exe" -ErrorAction SilentlyContinue
if (-not $makensis) {
    $commonPath = "C:\Program Files (x86)\NSIS\makensis.exe"
    if (Test-Path -LiteralPath $commonPath) {
        $makensis = Get-Item -LiteralPath $commonPath
    } else {
        throw "NSIS makensis.exe was not found. Install NSIS and run this script again."
    }
}

$defines = @(
    "/DVERSION=$Version",
    "/DBUILD_DIR=$buildDirectory",
    "/DOUTPUT_DIR=$outputDirectory",
    "/DPACKAGE_DIR=$packageDirectory"
)
& $makensis @defines (Join-Path $repoRoot "packaging\windows\agent.nsi")
if ($LASTEXITCODE -ne 0) { throw "Agent installer creation failed." }
& $makensis @defines (Join-Path $repoRoot "packaging\windows\debugger.nsi")
if ($LASTEXITCODE -ne 0) { throw "Debugger installer creation failed." }

Get-ChildItem -LiteralPath $outputDirectory -File |
    Select-Object Name, Length |
    Format-Table -AutoSize
