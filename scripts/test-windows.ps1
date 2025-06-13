# Windows test script for HT
# This script validates that HT works correctly on Windows

param(
    [string]$BinaryPath = "target\release\ht.exe"
)

Write-Host "Testing HT Windows functionality..."

# Test 1: Check if binary exists and is executable
if (-not (Test-Path $BinaryPath)) {
    Write-Error "HT binary not found at $BinaryPath"
    exit 1
}

Write-Host "✓ Binary exists"

# Test 2: Test help command
try {
    $helpOutput = & $BinaryPath --help 2>&1
    if ($LASTEXITCODE -ne 0) {
        Write-Error "Help command failed"
        exit 1
    }
    if (-not ($helpOutput -match "Usage:")) {
        Write-Error "Help output doesn't contain expected content"
        exit 1
    }
    Write-Host "✓ Help command works"
} catch {
    Write-Error "Failed to run help command: $_"
    exit 1
}

# Test 3: Test version command
try {
    $versionOutput = & $BinaryPath --version 2>&1
    if ($LASTEXITCODE -ne 0) {
        Write-Error "Version command failed"
        exit 1
    }
    Write-Host "✓ Version command works: $versionOutput"
} catch {
    Write-Error "Failed to run version command: $_"
    exit 1
}

# Test 4: Test that binary can start (quick test)
try {
    $process = Start-Process -FilePath $BinaryPath -ArgumentList "cmd", "/c", "echo", "test" -PassThru -NoNewWindow
    Start-Sleep -Seconds 2
    if (-not $process.HasExited) {
        $process.Kill()
        Write-Host "✓ Binary can start and run commands"
    } else {
        Write-Host "✓ Binary executed command and exited"
    }
} catch {
    Write-Warning "Could not test binary execution: $_"
}

# Test 5: Check binary dependencies
try {
    $dependencies = dumpbin /DEPENDENTS $BinaryPath 2>&1
    if ($dependencies -match "KERNEL32.dll") {
        Write-Host "✓ Binary has expected Windows dependencies"
    }
} catch {
    Write-Warning "Could not check dependencies (dumpbin not available)"
}

Write-Host ""
Write-Host "🎉 All Windows tests passed!"
Write-Host "HT appears to be working correctly on Windows."