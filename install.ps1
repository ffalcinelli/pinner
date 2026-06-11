$ErrorActionPreference = 'Stop'

$Repo = "ffalcinelli/pinner"
$GithubUrl = "https://github.com/$Repo"

# Detect Architecture
$Arch = if ($env:PROCESSOR_ARCHITECTURE -eq 'AMD64') { "amd64" } elseif ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64') { "arm64" } else {
    Write-Error "Unsupported architecture: $($env:PROCESSOR_ARCHITECTURE)"
    exit 1
}

$AssetName = "pinner-windows-$Arch"
$Extension = "zip"

# Determine install directory
$InstallDir = if (Test-Path "$HOME\.cargo\bin") {
    "$HOME\.cargo\bin"
} elseif (Test-Path "$HOME\.local\bin") {
    "$HOME\.local\bin"
} else {
    $dir = "$HOME\.local\bin"
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    $dir
}

Write-Host "Installing pinner to $InstallDir..."

# Get latest release tag
$ReleaseInfo = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest"
$LatestRelease = $ReleaseInfo.tag_name

if (-not $LatestRelease) {
    Write-Error "Could not determine latest release version."
    exit 1
}

$DownloadUrl = "$GithubUrl/releases/download/$LatestRelease/$AssetName.$Extension"

# Create a temporary directory
$TempDir = Join-Path $env:TEMP ([Guid]::NewGuid().ToString())
New-Item -ItemType Directory -Path $TempDir | Out-Null

try {
    $ZipPath = Join-Path $TempDir "pinner.zip"
    Write-Host "Downloading $DownloadUrl..."
    Invoke-WebRequest -Uri $DownloadUrl -OutFile $ZipPath

    # Extract
    Expand-Archive -Path $ZipPath -DestinationPath $TempDir -Force

    # Move to install directory
    $ExePath = Join-Path $TempDir "pinner.exe"
    Move-Item -Path $ExePath -Destination (Join-Path $InstallDir "pinner.exe") -Force

    Write-Host "pinner $LatestRelease installed successfully to $InstallDir"

    # Check if InstallDir is in PATH
    $UserPath = [Environment]::GetEnvironmentVariable("Path", "User")
    if ($UserPath -notlike "*$InstallDir*") {
        Write-Host ""
        Write-Host "Warning: $InstallDir is not in your PATH." -ForegroundColor Yellow
        Write-Host "You can add it by running:"
        Write-Host "  [Environment]::SetEnvironmentVariable('Path', `$UserPath + ';$InstallDir', 'User')"
        Write-Host "Then restart your terminal."
    }
} finally {
    Remove-Item -Path $TempDir -Recurse -Force
}
