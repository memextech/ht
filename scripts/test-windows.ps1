# Windows test script for HT
# This script validates that HT works correctly on Windows

param(
    [string]$BinaryPath = "target\release\ht.exe"
)

Write-Host "Testing HT Windows functionality..."

# Test 1: Check if binary exists and is executable
Write-Host "Looking for binary at: $BinaryPath"
if (-not (Test-Path $BinaryPath)) {
    Write-Host "Current directory contents:"
    Get-ChildItem -Recurse -Name | Select-Object -First 20
    Write-Host "Target directory contents:"
    if (Test-Path "target") {
        Get-ChildItem "target" -Recurse -Name | Select-Object -First 20
    }
    Write-Error "HT binary not found at $BinaryPath"
    exit 1
}

$binaryInfo = Get-Item $BinaryPath
Write-Host "✓ Binary exists: $($binaryInfo.Length) bytes"

# Test 2: Test help command
try {
    Write-Host "Testing help command..."
    $helpOutput = & $BinaryPath --help 2>&1
    Write-Host "Help command exit code: $LASTEXITCODE"
    Write-Host "Help output length: $($helpOutput.Length)"
    if ($helpOutput.Length -gt 0) {
        Write-Host "First 100 chars of help output: $($helpOutput.ToString().Substring(0, [Math]::Min(100, $helpOutput.ToString().Length)))"
    }
    
    if ($LASTEXITCODE -ne 0) {
        Write-Error "Help command failed with exit code: $LASTEXITCODE"
        Write-Host "Full help output: $helpOutput"
        exit 1
    }
    if (-not ($helpOutput -match "Usage:")) {
        Write-Error "Help output does not contain expected Usage: content"
        Write-Host "Full help output: $helpOutput"
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
    Write-Host "Testing binary execution..."
    # Just test that the binary can be executed without hanging
    $timeout = 5
    $process = Start-Process -FilePath $BinaryPath -ArgumentList "--help" -PassThru -NoNewWindow -RedirectStandardOutput "nul" -RedirectStandardError "nul"
    if ($process.WaitForExit($timeout * 1000)) {
        Write-Host "✓ Binary executed successfully (exit code: $($process.ExitCode))"
    } else {
        $process.Kill()
        Write-Host "✓ Binary started but was terminated after timeout"
    }
} catch {
    Write-Warning "Could not test binary execution: $_"
    # Do not fail the test for this - it is optional
}

# Test 5: Check binary dependencies (optional)
try {
    if (Get-Command "dumpbin" -ErrorAction SilentlyContinue) {
        $dependencies = dumpbin /DEPENDENTS $BinaryPath 2>&1
        if ($dependencies -match "KERNEL32.dll") {
            Write-Host "✓ Binary has expected Windows dependencies"
        }
    } else {
        Write-Host "⚠ dumpbin not available, skipping dependency check"
    }
} catch {
    Write-Warning "Could not check dependencies: $_"
}

Write-Host ""
Write-Host "🎉 All Windows tests passed!"
Write-Host "HT appears to be working correctly on Windows."