param(
    [string]$Version = $env:CODETRAIL_VERSION,
    [string]$Repo = $env:CODETRAIL_REPO,
    [string]$InstallDir = $env:CODETRAIL_INSTALL_DIR,
    [switch]$DryRun
)

$ErrorActionPreference = "Stop"

if ([string]::IsNullOrWhiteSpace($Version)) {
    $Version = "latest"
}
if ([string]::IsNullOrWhiteSpace($Repo)) {
    $Repo = "mars167/CodeTrail"
}
if ([string]::IsNullOrWhiteSpace($InstallDir)) {
    $localAppData = $env:LOCALAPPDATA
    if ([string]::IsNullOrWhiteSpace($localAppData)) {
        $localAppData = Join-Path $HOME ".local"
    }
    $InstallDir = Join-Path $localAppData "Programs\codetrail\bin"
}
if ($env:CODETRAIL_DRY_RUN -eq "1") {
    $DryRun = $true
}

function Get-CodeTrailArchitecture {
    $arch = $env:CODETRAIL_ARCH
    if ([string]::IsNullOrWhiteSpace($arch)) {
        $arch = $env:PROCESSOR_ARCHITEW6432
    }
    if ([string]::IsNullOrWhiteSpace($arch)) {
        $arch = $env:PROCESSOR_ARCHITECTURE
    }
    if ([string]::IsNullOrWhiteSpace($arch)) {
        $arch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
    }

    switch -Regex ($arch) {
        "^(X64|x86_64|amd64)$" { return "amd64" }
        "^(AMD64)$" { return "amd64" }
        "^(Arm64|ARM64|arm64|aarch64)$" { return "arm64" }
        default { throw "Unsupported architecture: $arch" }
    }
}

$assetArch = Get-CodeTrailArchitecture
$asset = "codetrail-windows-$assetArch.exe.zip"
if ($Version -eq "latest") {
    $baseUrl = "https://github.com/$Repo/releases/latest/download"
} else {
    $baseUrl = "https://github.com/$Repo/releases/download/$Version"
}

if ($DryRun) {
    Write-Output "repo=$Repo"
    Write-Output "version=$Version"
    Write-Output "asset=$asset"
    Write-Output "install_dir=$InstallDir"
    Write-Output "url=$baseUrl/$asset"
    return
}

$tmpDir = Join-Path ([System.IO.Path]::GetTempPath()) ("codetrail-install-" + [System.Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $tmpDir | Out-Null

try {
    $assetPath = Join-Path $tmpDir $asset
    $checksumsPath = Join-Path $tmpDir "SHA256SUMS"

    Write-Output "Downloading $asset..."
    Invoke-WebRequest -Uri "$baseUrl/$asset" -OutFile $assetPath
    Invoke-WebRequest -Uri "$baseUrl/SHA256SUMS" -OutFile $checksumsPath

    $expected = $null
    foreach ($line in Get-Content $checksumsPath) {
        $parts = $line -split "\s+"
        if ($parts.Length -ge 2 -and $parts[1] -eq $asset) {
            $expected = $parts[0].ToLowerInvariant()
            break
        }
    }
    if ([string]::IsNullOrWhiteSpace($expected)) {
        throw "Checksum for $asset was not found in SHA256SUMS."
    }

    $actual = (Get-FileHash -Algorithm SHA256 -Path $assetPath).Hash.ToLowerInvariant()
    if ($actual -ne $expected) {
        throw "Checksum mismatch for $asset. Expected $expected, got $actual."
    }

    $extractDir = Join-Path $tmpDir "extract"
    Expand-Archive -Path $assetPath -DestinationPath $extractDir -Force
    $exePath = Join-Path $extractDir "codetrail.exe"
    if (-not (Test-Path $exePath)) {
        throw "Release archive did not contain codetrail.exe."
    }

    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    $installedExe = Join-Path $InstallDir "codetrail.exe"
    Copy-Item -Path $exePath -Destination $installedExe -Force

    # Run the installed binary once so a broken executable fails loudly here
    # instead of silently printing nothing later. Relax the error preference so
    # Windows PowerShell 5.1 does not turn stderr lines into terminating errors
    # before we can report the exit code.
    $previousErrorActionPreference = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    $versionOutput = & $installedExe --version 2>&1
    $ErrorActionPreference = $previousErrorActionPreference
    if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace("$versionOutput")) {
        $code = $LASTEXITCODE
        $hex = "0x{0:X8}" -f $code
        $hint = switch ($code) {
            -1073741515 { "The Windows loader could not start the binary (STATUS_DLL_NOT_FOUND). A required runtime DLL such as VCRUNTIME140.dll is missing. Releases v0.1.6-beta.2 and earlier require the Microsoft Visual C++ Redistributable; newer releases are fully self-contained." }
            -1073741795 { "The binary uses CPU instructions this machine cannot execute (STATUS_ILLEGAL_INSTRUCTION). You may have installed the wrong architecture; set CODETRAIL_ARCH=arm64 or amd64 and reinstall." }
            default { "The binary started but failed immediately. Check the architecture ($assetArch) matches this machine and that antivirus is not blocking it." }
        }
        throw "codetrail.exe failed its post-install check (exit code $code / $hex). $hint"
    }
    Write-Output "Verified: $versionOutput"

    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    $pathParts = @()
    if (-not [string]::IsNullOrWhiteSpace($userPath)) {
        $pathParts = $userPath -split ";"
    }
    if ($pathParts -notcontains $InstallDir) {
        $newUserPath = if ([string]::IsNullOrWhiteSpace($userPath)) { $InstallDir } else { "$userPath;$InstallDir" }
        [Environment]::SetEnvironmentVariable("Path", $newUserPath, "User")
    }
    if (($env:Path -split ";") -notcontains $InstallDir) {
        $env:Path = "$env:Path;$InstallDir"
    }

    Write-Output "Installed codetrail to $(Join-Path $InstallDir 'codetrail.exe')"
    Write-Output "Restart your terminal if codetrail is not found immediately."
}
finally {
    Remove-Item -Recurse -Force $tmpDir -ErrorAction SilentlyContinue
}
