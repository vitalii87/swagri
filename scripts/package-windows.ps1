param(
    [string]$Version = "0.15.0-alpha",
    [string]$Configuration = "release",
    [string]$RuntimeDirectory = "",
    [string]$AndroidApk = ""
)

$ErrorActionPreference = "Stop"
$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
$buildDirectory = Join-Path $repoRoot "target\$Configuration"
$distRoot = Join-Path $repoRoot "dist"
if ($Version -notmatch '^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$') {
    throw "Version contains unsupported path characters: $Version"
}
$outputDirectory = Join-Path $distRoot $Version
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
$resolvedDistRoot = [System.IO.Path]::GetFullPath($distRoot).TrimEnd('\') + '\'
if (-not $resolvedOutput.StartsWith($resolvedDistRoot, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Refusing to package outside the versioned dist directory."
}
New-Item -ItemType Directory -Path $packageDirectory -Force | Out-Null

$readme = @"
Swagri $Version experimental build

Debugger package:
  Run swagri-debugger.exe. It starts the bundled agent, shows host metrics,
  peer capacity scores, offers smart CPU and matrix tests, can pause this
  computer's Swagri contribution, and keeps a persistent SQLite task history.

Agent package:
  Run swagri-agent.exe --name <device-name>
  Type help for commands. The persistent identity is stored under LocalAppData.
  Resource sampling defaults to 5 seconds; CPU calibration is cached after one run.

Artifacts: Debugger can split files into immutable SHA-256 blocks in a local
content-addressed store. The default disk contribution limit is 5%.
Trusted peers can list artifacts and resume-download only missing verified blocks.

Updates: both packages include swagri-updater.exe. Trust a specific Peer ID
before receiving signed P2P updates. Debugger packages can share both Agent and
GUI updates; headless packages share Agent only. Use trusted test networks.
When Swagri-Android-Agent.apk is included, the Debugger Agent can also serve it
to trusted Android peers; Android verifies it and requires install confirmation.
Documentation: https://github.com/vitalii87/swagri
"@
Set-Content -LiteralPath (Join-Path $packageDirectory "README.txt") -Value $readme -Encoding utf8
$binaryVersion = $Version.Split('-')[0]
Set-Content -LiteralPath (Join-Path $packageDirectory "swagri-debugger.version") -Value $binaryVersion -Encoding ascii

$runtimeFiles = @()
if ($RuntimeDirectory) {
    $resolvedRuntimeDirectory = [System.IO.Path]::GetFullPath($RuntimeDirectory)
    $libunwind = Join-Path $resolvedRuntimeDirectory "libunwind.dll"
    if (-not (Test-Path -LiteralPath $libunwind)) {
        throw "Runtime directory was supplied, but libunwind.dll was not found: $libunwind"
    }
    Copy-Item -LiteralPath $libunwind -Destination $packageDirectory -Force
    $runtimeFiles += "libunwind.dll"
}

$androidApkSource = $null
if ($AndroidApk) {
    $androidApkSource = [System.IO.Path]::GetFullPath($AndroidApk)
    if (-not (Test-Path -LiteralPath $androidApkSource -PathType Leaf)) {
        throw "Android APK was supplied, but the file was not found: $androidApkSource"
    }
    Copy-Item -LiteralPath $androidApkSource -Destination (Join-Path $packageDirectory "Swagri-Android-Agent.apk") -Force
}

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
if ($androidApkSource) {
    Copy-Item -LiteralPath (Join-Path $packageDirectory "Swagri-Android-Agent.apk") -Destination $debuggerPortable -Force
}
foreach ($runtimeFile in $runtimeFiles) {
    Copy-Item -LiteralPath (Join-Path $packageDirectory $runtimeFile) -Destination $agentPortable -Force
    Copy-Item -LiteralPath (Join-Path $packageDirectory $runtimeFile) -Destination $debuggerPortable -Force
}

Compress-Archive -Path (Join-Path $agentPortable "*") -DestinationPath (Join-Path $outputDirectory "Swagri-Agent-Portable-x64.zip") -Force
Compress-Archive -Path (Join-Path $debuggerPortable "*") -DestinationPath (Join-Path $outputDirectory "Swagri-Debugger-Portable-x64.zip") -Force

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
if ($runtimeFiles -contains "libunwind.dll") {
    $defines += "/DRUNTIME_DLL=$packageDirectory\libunwind.dll"
}
if ($androidApkSource) {
    $defines += "/DANDROID_APK=$packageDirectory\Swagri-Android-Agent.apk"
}
& $makensis @defines (Join-Path $repoRoot "packaging\windows\agent.nsi")
if ($LASTEXITCODE -ne 0) { throw "Agent installer creation failed." }
& $makensis @defines (Join-Path $repoRoot "packaging\windows\debugger.nsi")
if ($LASTEXITCODE -ne 0) { throw "Debugger installer creation failed." }

Get-ChildItem -LiteralPath $outputDirectory -File |
    Select-Object Name, Length |
    Format-Table -AutoSize
