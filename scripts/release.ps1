$ErrorActionPreference = "Stop"

$projectRoot = Split-Path $PSScriptRoot -Parent
$releaseDir = Join-Path $projectRoot "release"
$cargoRelease = Join-Path (Join-Path $projectRoot "dist") "target"
$cargoRelease = Join-Path $cargoRelease "release"

$agentsDir = Join-Path $releaseDir "agents"
$toolsDir = Join-Path $releaseDir "tools"
$commandsDir = Join-Path $releaseDir "commands"

if (Test-Path $releaseDir) { Remove-Item -Recurse -Force $releaseDir }
New-Item -ItemType Directory -Force -Path $releaseDir | Out-Null
New-Item -ItemType Directory -Force -Path $agentsDir | Out-Null
New-Item -ItemType Directory -Force -Path $toolsDir | Out-Null
New-Item -ItemType Directory -Force -Path $commandsDir | Out-Null

Copy-Item (Join-Path $cargoRelease "scarllet-core.exe") (Join-Path $releaseDir "core.exe")
Copy-Item (Join-Path $cargoRelease "scarllet-tui.exe") (Join-Path $releaseDir "tui.exe")
Copy-Item (Join-Path $cargoRelease "default-agent.exe") (Join-Path $agentsDir "default.exe")
Copy-Item (Join-Path $cargoRelease "terminal-tool.exe") (Join-Path $toolsDir "terminal.exe")
Copy-Item (Join-Path $cargoRelease "find-tool.exe") (Join-Path $toolsDir "find.exe")
Copy-Item (Join-Path $cargoRelease "grep-tool.exe") (Join-Path $toolsDir "grep.exe")
Copy-Item (Join-Path $cargoRelease "edit-tool.exe") (Join-Path $toolsDir "edit.exe")
Copy-Item (Join-Path $cargoRelease "write-tool.exe") (Join-Path $toolsDir "write.exe")

Write-Host "Release folder created at: $releaseDir"
Get-ChildItem -Recurse $releaseDir | ForEach-Object {
    Write-Host ("  " + $_.FullName.Replace($releaseDir, "release"))
}
