# Fails when a Windows .exe imports the Visual C++ runtime DLLs
# (VCRUNTIME*, MSVCP*, MSVCR*, CONCRT*, VCOMP*, VCCORLIB*), which a clean
# Windows doesn't have, or the Universal CRT ones (ucrtbase.dll,
# api-ms-win-crt-*.dll). The release and CI workflows run it on tmpmail.exe,
# built with `-C target-feature=+crt-static`, which links both in.
#
#   pwsh scripts/check-windows-imports.ps1 target/release/tmpmail.exe
#
# -AllowUcrt accepts the Universal CRT (part of Windows 10 and later), for a
# binary that links only the VC runtime statically, as Tauri's
# staticVCRuntime does.
#
# Uses dumpbin from the Visual Studio install that vswhere finds (GitHub's
# Windows runners have one), so it needs no MSVC dev shell.
param([Parameter(Mandatory)][string]$Exe, [switch]$AllowUcrt)
$ErrorActionPreference = 'Stop'

if (-not (Test-Path -LiteralPath $Exe -PathType Leaf)) {
  throw "no such file: $Exe"
}

$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
if (-not (Test-Path -LiteralPath $vswhere)) {
  throw "vswhere not found at $vswhere"
}
$dumpbin = & $vswhere -latest -products * `
  -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
  -find 'VC\Tools\MSVC\**\bin\Hostx64\x64\dumpbin.exe' |
  Sort-Object -Descending | Select-Object -First 1  # the newest toolset
if (-not $dumpbin) {
  throw 'dumpbin not found (no Visual Studio with the x64 C++ tools)'
}

$out = & $dumpbin /nologo /dependents $Exe
if ($LASTEXITCODE -ne 0) {
  $out | Write-Host
  throw "dumpbin exited with $LASTEXITCODE"
}

# Both the import and delay-load lists are DLL names on their own line.
$dlls = @($out | ForEach-Object { $_.Trim() } | Where-Object { $_ -match '\.dll$' })
Write-Host "$Exe imports:"
$dlls | ForEach-Object { Write-Host "  $_" }

# Every Windows .exe imports KERNEL32.dll; without it the parse went wrong,
# and an empty list must not pass.
if (-not ($dlls -contains 'KERNEL32.dll')) {
  throw 'no KERNEL32.dll in the import list; dumpbin output not understood'
}

$pattern = '^(vcruntime|msvcp|msvcr|concrt|vcomp|vccorlib)'
if (-not $AllowUcrt) { $pattern += '|^(ucrtbase|api-ms-win-crt-)' }
$bad = @($dlls | Where-Object { $_ -match $pattern })
if ($bad.Count -gt 0) {
  Write-Host "::error::$Exe needs the C runtime DLLs: $($bad -join ', ')"
  exit 1
}
Write-Host 'No C runtime DLL imports'
