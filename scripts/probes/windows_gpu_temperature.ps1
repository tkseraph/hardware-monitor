param(
    [Parameter(Mandatory=$true)][string]$SdkPath,
    [string]$OutputDirectory = (Join-Path $env:TEMP ('hardware-monitor-gpu-probe-' + [guid]::NewGuid().ToString('N'))),
    [switch]$BuildOnly
)
$ErrorActionPreference = 'Stop'
if (-not [Environment]::Is64BitProcess) { throw 'Run with 64-bit PowerShell.' }
$sdkRoot = (Resolve-Path -LiteralPath $SdkPath).Path
$sdkCommit = (& git -C $sdkRoot rev-parse HEAD).Trim()
if ($LASTEXITCODE -ne 0 -or $sdkCommit -ne 'd9f04a9bba022d6cf6333f005dd540b4ad19fb63') {
    throw 'Expected the official ADLX v1.5 SDK commit.'
}
& git -C $sdkRoot diff --exit-code HEAD -- SDK/Include | Out-Null
if ($LASTEXITCODE -ne 0) { throw 'SDK headers have local modifications.' }
$vsWhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
$vsRoot = (& $vsWhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath).Trim()
if (-not $vsRoot) { throw 'Microsoft C++ Build Tools not found.' }
$vcVersion = Get-ChildItem -LiteralPath (Join-Path $vsRoot 'VC\Tools\MSVC') -Directory | Sort-Object { [version]$_.Name } -Descending | Select-Object -First 1
$vcRoot = $vcVersion.FullName
$windowsKitRoot = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10'
$kitVersion = Get-ChildItem -LiteralPath (Join-Path $windowsKitRoot 'Include') -Directory | Sort-Object { [version]$_.Name } -Descending | Select-Object -First 1
$kitInclude = Join-Path $windowsKitRoot ('Include\' + $kitVersion.Name)
$kitLib = Join-Path $windowsKitRoot ('Lib\' + $kitVersion.Name)
New-Item -ItemType Directory -Path $OutputDirectory -Force | Out-Null
$outputRoot = (Resolve-Path -LiteralPath $OutputDirectory).Path
$probeExe = Join-Path $outputRoot 'gpu-temperature-probe.exe'
$source = Join-Path $PSScriptRoot 'windows_gpu_temperature.cpp'
$compiler = Join-Path $vcRoot 'bin\Hostx64\x64\cl.exe'
$compilerArgs = @('/nologo','/std:c++17','/EHsc','/W4','/wd4068','/O2','/MT','/utf-8',
    ('/I' + (Join-Path $sdkRoot 'SDK\Include')),('/I' + (Join-Path $vcRoot 'include')),
    ('/I' + (Join-Path $kitInclude 'ucrt')),('/I' + (Join-Path $kitInclude 'shared')),('/I' + (Join-Path $kitInclude 'um')),
    ('/I' + (Join-Path $kitInclude 'winrt')),
    ('/Fo' + (Join-Path $outputRoot 'gpu-temperature-probe.obj')),('/Fe' + $probeExe),$source,
    '/link',('/LIBPATH:' + (Join-Path $vcRoot 'lib\x64')),('/LIBPATH:' + (Join-Path $kitLib 'ucrt\x64')),
    ('/LIBPATH:' + (Join-Path $kitLib 'um\x64')),'dxgi.lib')
& $compiler @compilerArgs
if ($LASTEXITCODE -ne 0) { throw 'GPU temperature probe build failed.' }
if ($BuildOnly) { Write-Output $probeExe; return }
# Check the existing driver runtime before launching the bounded ordinary process.
$driverDll = Join-Path ([Environment]::SystemDirectory) 'amdadlx64.dll'
$signature = Get-AuthenticodeSignature -LiteralPath $driverDll
if ($signature.Status -ne 'Valid') { throw 'The system ADLX runtime signature is not valid.' }
$start = [Diagnostics.ProcessStartInfo]::new()
$start.FileName = $probeExe
$start.WorkingDirectory = $outputRoot
$start.UseShellExecute = $false
$start.CreateNoWindow = $true
$start.RedirectStandardOutput = $true
$start.RedirectStandardError = $true
$probeProcess = [Diagnostics.Process]::Start($start)
$stdout = $probeProcess.StandardOutput.ReadToEndAsync()
$stderr = $probeProcess.StandardError.ReadToEndAsync()
try {
    if (-not $probeProcess.WaitForExit(15000)) {
        $probeProcess.Kill()
        $probeProcess.WaitForExit()
        throw 'GPU temperature probe timed out and was stopped.'
    }
    $raw = $stdout.GetAwaiter().GetResult()
    $null = $stderr.GetAwaiter().GetResult()
    if ($probeProcess.ExitCode -ne 0) { throw 'GPU temperature probe did not cleanly terminate ADLX.' }
    if ([Text.Encoding]::UTF8.GetByteCount($raw) -gt 32768) { throw 'GPU probe output exceeded 32 KiB.' }
    $report = $raw | ConvertFrom-Json
    $report | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath (Join-Path $outputRoot 'gpu-temperature-result.json') -Encoding UTF8
    Write-Output $raw
} finally {
    if (-not $probeProcess.HasExited) { $probeProcess.Kill(); $probeProcess.WaitForExit() }
    $probeProcess.Dispose()
}
