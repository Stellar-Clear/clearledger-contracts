#!/usr/bin/env pwsh
# verify.ps1 — Post-deployment verification script for ClearLedger contracts (PowerShell)
#
# Usage:
#   powershell ./scripts/verify.ps1
#
# Mirrors scripts/verify.sh: checks required env vars are set, then runs a
# read-only query against each deployed contract to confirm it responds.

$ErrorActionPreference = 'Stop'

# ---------------------------------------------------------------------------
# 0. CLI / env setup
# ---------------------------------------------------------------------------

$stellar = Get-Command stellar -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Source
if (-not $stellar) {
    $stellarBin = [Environment]::GetEnvironmentVariable('STELLAR_BIN')
    if ($stellarBin -and (Test-Path $stellarBin)) {
        $stellar = $stellarBin
    }
    elseif (Test-Path "${env:ProgramFiles(x86)}\Stellar CLI\stellar.exe") {
        $stellar = "${env:ProgramFiles(x86)}\Stellar CLI\stellar.exe"
    }
    else {
        Write-Host "Error: stellar CLI not found."
        Write-Host ""
        Write-Host "Try one of:"
        Write-Host "  1. Install stellar CLI globally (https://developers.stellar.org/docs/learn/developing-with-soroban/setup)"
        Write-Host "  2. Set STELLAR_BIN=/path/to/stellar.exe"
        Write-Host "  3. Ensure 'Stellar CLI' is installed in Program Files (x86)"
        Write-Host ""
        exit 1
    }
}

if (-not (Test-Path .env)) {
    Write-Host "Error: .env file not found."
    Write-Host "Run ./scripts/deploy.ps1 first to create the .env file with contract IDs."
    exit 1
}

Get-Content .env | ForEach-Object {
    if ($_ -match '^([^=]+)=(.*)$') {
        [Environment]::SetEnvironmentVariable($matches[1], $matches[2])
    }
}

$script:failed = $false

# ---------------------------------------------------------------------------
# 1. Required env vars
# ---------------------------------------------------------------------------

function Test-EnvVar {
    param($name)
    $val = [Environment]::GetEnvironmentVariable($name)
    if (-not $val) {
        Write-Host "Error: $name is not set or is empty."
        $script:failed = $true
    }
}

Write-Host "=== Verifying Contract Deployment Configuration ==="
Test-EnvVar "DEPLOYER_ACCOUNT"
Test-EnvVar "REGISTRY_CONTRACT_ID"
Test-EnvVar "INVOICE_CONTRACT_ID"
Test-EnvVar "POOL_USDC_CONTRACT_ID"
Test-EnvVar "POOL_XLM_CONTRACT_ID"
Test-EnvVar "ESCROW_USDC_CONTRACT_ID"
Test-EnvVar "ESCROW_XLM_CONTRACT_ID"

if ($script:failed) {
    Write-Host "Configuration check failed. Run ./scripts/deploy.ps1 first to populate contract IDs."
    exit 1
}

$deployerAccount = [Environment]::GetEnvironmentVariable('DEPLOYER_ACCOUNT')
$registryId = [Environment]::GetEnvironmentVariable('REGISTRY_CONTRACT_ID')
$invoiceId = [Environment]::GetEnvironmentVariable('INVOICE_CONTRACT_ID')
$poolUsdcId = [Environment]::GetEnvironmentVariable('POOL_USDC_CONTRACT_ID')
$poolXlmId = [Environment]::GetEnvironmentVariable('POOL_XLM_CONTRACT_ID')
$escrowUsdcId = [Environment]::GetEnvironmentVariable('ESCROW_USDC_CONTRACT_ID')
$escrowXlmId = [Environment]::GetEnvironmentVariable('ESCROW_XLM_CONTRACT_ID')

# ---------------------------------------------------------------------------
# 2. Verification helper
# ---------------------------------------------------------------------------

function Invoke-VerifyCheck {
    param($name, [string[]]$queryArgs)

    Write-Host "Verifying $name..."
    $output = & $stellar contract invoke @queryArgs 2>&1
    if ($LASTEXITCODE -eq 0) {
        Write-Host "  [PASS] $name"
        Write-Host "  Result: $output"
    } else {
        Write-Host "  [FAIL] $name"
        Write-Host "  Error: $output"
        $script:failed = $true
    }
    Write-Host ""
}

Write-Host "=== Running Contract Verification Queries ==="

# 1. Registry Contract - get_admin
Invoke-VerifyCheck "registry_contract (get_admin)" @("--id", $registryId, "--source", $deployerAccount, "--network", "testnet", "--", "get_admin")

# 2. Invoice Contract - get_counts
Invoke-VerifyCheck "invoice_contract (get_counts)" @("--id", $invoiceId, "--source", $deployerAccount, "--network", "testnet", "--", "get_counts")

# 3. USDC Pool Contract - get_stats
Invoke-VerifyCheck "pool_usdc_contract (get_stats)" @("--id", $poolUsdcId, "--source", $deployerAccount, "--network", "testnet", "--", "get_stats")

# 4. XLM Pool Contract - get_stats
Invoke-VerifyCheck "pool_xlm_contract (get_stats)" @("--id", $poolXlmId, "--source", $deployerAccount, "--network", "testnet", "--", "get_stats")

# 5. USDC Escrow Contract - get_locked (confirm existence with dummy ID)
Invoke-VerifyCheck "escrow_usdc_contract (get_locked)" @("--id", $escrowUsdcId, "--source", $deployerAccount, "--network", "testnet", "--", "get_locked", "--invoice_id", "0000000000000000000000000000000000000000000000000000000000000000")

# 6. XLM Escrow Contract - get_locked (confirm existence with dummy ID)
Invoke-VerifyCheck "escrow_xlm_contract (get_locked)" @("--id", $escrowXlmId, "--source", $deployerAccount, "--network", "testnet", "--", "get_locked", "--invoice_id", "0000000000000000000000000000000000000000000000000000000000000000")

if ($script:failed) {
    Write-Host "Verification failed."
    exit 1
} else {
    Write-Host "All contract verifications passed successfully."
    exit 0
}
