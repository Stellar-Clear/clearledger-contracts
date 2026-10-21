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
Test-EnvVar "ESCROW_USDC_CONTRACT_ID"
Test-EnvVar "USDC_ISSUER"
if ([Environment]::GetEnvironmentVariable('DEPLOY_XLM') -eq 'true') {
    Test-EnvVar "POOL_XLM_CONTRACT_ID"
    Test-EnvVar "ESCROW_XLM_CONTRACT_ID"
    Test-EnvVar "XLM_ASSET"
}

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
$usdcAsset = [Environment]::GetEnvironmentVariable('USDC_ISSUER')
$xlmAsset = [Environment]::GetEnvironmentVariable('XLM_ASSET')

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

function Invoke-VerifyValue {
    param([string]$name, [string]$expected, [string[]]$queryArgs)

    Write-Host "Verifying $name..."
    $output = (& $stellar contract invoke @queryArgs 2>&1 | Out-String)
    if ($LASTEXITCODE -eq 0 -and $output.Contains($expected)) {
        Write-Host "  [PASS] $name"
    } else {
        Write-Host "  [FAIL] $name"
        Write-Host "  Expected: $expected"
        Write-Host "  Result: $output"
        $script:failed = $true
    }
    Write-Host ""
}

function Invoke-VerifyWasmHash {
    param([string]$name, [string]$contractId, [string]$wasmPath)

    Write-Host "Verifying $name WASM hash..."
    if (-not (Test-Path $wasmPath)) {
        Write-Host "  [FAIL] Local WASM not found: $wasmPath"
        $script:failed = $true
        return
    }

    $info = (& $stellar contract info --id $contractId --network testnet 2>&1 | Out-String)
    $hashMatches = [regex]::Matches($info, '(?i)[0-9a-f]{64}')
    $expectedHash = (Get-FileHash $wasmPath -Algorithm SHA256).Hash.ToLowerInvariant()
    $actualHash = if ($hashMatches.Count -gt 0) { $hashMatches[$hashMatches.Count - 1].Value.ToLowerInvariant() } else { '' }
    if ($LASTEXITCODE -eq 0 -and $actualHash -eq $expectedHash) {
        Write-Host "  [PASS] $name WASM hash ($actualHash)"
    } else {
        Write-Host "  [FAIL] $name WASM hash"
        Write-Host "  Expected: $expectedHash"
        Write-Host "  Result: $info"
        $script:failed = $true
    }
    Write-Host ""
}

Write-Host "=== Running Contract Verification Queries ==="

$deployerAddress = & $stellar keys address $deployerAccount 2>&1
if ($LASTEXITCODE -ne 0) {
    Write-Host "Error: could not resolve DEPLOYER_ACCOUNT '$deployerAccount'."
    exit 1
}

Invoke-VerifyWasmHash "registry_contract" $registryId "target/wasm32v1-none/release/clearledger_registry.wasm"
Invoke-VerifyWasmHash "invoice_contract" $invoiceId "target/wasm32v1-none/release/clearledger_invoice.wasm"
Invoke-VerifyWasmHash "pool_usdc_contract" $poolUsdcId "target/wasm32v1-none/release/clearledger_pool.wasm"
Invoke-VerifyWasmHash "escrow_usdc_contract" $escrowUsdcId "target/wasm32v1-none/release/clearledger_escrow.wasm"

Invoke-VerifyValue "registry admin" $deployerAddress @("--id", $registryId, "--source", $deployerAccount, "--network", "testnet", "--", "get_admin")
Invoke-VerifyValue "invoice admin" $deployerAddress @("--id", $invoiceId, "--source", $deployerAccount, "--network", "testnet", "--", "get_admin")
Invoke-VerifyValue "USDC pool admin" $deployerAddress @("--id", $poolUsdcId, "--source", $deployerAccount, "--network", "testnet", "--", "get_admin")
Invoke-VerifyValue "USDC escrow admin" $deployerAddress @("--id", $escrowUsdcId, "--source", $deployerAccount, "--network", "testnet", "--", "get_admin")

Invoke-VerifyValue "invoice registry reference" $registryId @("--id", $invoiceId, "--source", $deployerAccount, "--network", "testnet", "--", "get_registry_contract")
Invoke-VerifyValue "invoice pool reference" $poolUsdcId @("--id", $invoiceId, "--source", $deployerAccount, "--network", "testnet", "--", "get_pool_contract")
Invoke-VerifyValue "invoice escrow reference" $escrowUsdcId @("--id", $invoiceId, "--source", $deployerAccount, "--network", "testnet", "--", "get_escrow_contract")
Invoke-VerifyValue "USDC pool invoice reference" $invoiceId @("--id", $poolUsdcId, "--source", $deployerAccount, "--network", "testnet", "--", "get_invoice_contract")
Invoke-VerifyValue "USDC pool escrow reference" $escrowUsdcId @("--id", $poolUsdcId, "--source", $deployerAccount, "--network", "testnet", "--", "get_escrow_contract")
Invoke-VerifyValue "USDC pool asset" $usdcAsset @("--id", $poolUsdcId, "--source", $deployerAccount, "--network", "testnet", "--", "get_usdc_asset")
Invoke-VerifyValue "USDC escrow pool reference" $poolUsdcId @("--id", $escrowUsdcId, "--source", $deployerAccount, "--network", "testnet", "--", "get_pool_contract")
Invoke-VerifyValue "USDC escrow asset" $usdcAsset @("--id", $escrowUsdcId, "--source", $deployerAccount, "--network", "testnet", "--", "get_usdc_asset")

# 1. Registry Contract - get_admin
Invoke-VerifyCheck "registry_contract (get_admin)" @("--id", $registryId, "--source", $deployerAccount, "--network", "testnet", "--", "get_admin")

# 2. Invoice Contract - get_counts
Invoke-VerifyCheck "invoice_contract (get_counts)" @("--id", $invoiceId, "--source", $deployerAccount, "--network", "testnet", "--", "get_counts")

# 3. USDC Pool Contract - get_stats
Invoke-VerifyCheck "pool_usdc_contract (get_stats)" @("--id", $poolUsdcId, "--source", $deployerAccount, "--network", "testnet", "--", "get_stats")

if ([Environment]::GetEnvironmentVariable('DEPLOY_XLM') -eq 'true') {
    Invoke-VerifyWasmHash "pool_xlm_contract" $poolXlmId "target/wasm32v1-none/release/clearledger_pool.wasm"
    Invoke-VerifyWasmHash "escrow_xlm_contract" $escrowXlmId "target/wasm32v1-none/release/clearledger_escrow.wasm"
    Invoke-VerifyValue "XLM pool admin" $deployerAddress @("--id", $poolXlmId, "--source", $deployerAccount, "--network", "testnet", "--", "get_admin")
    Invoke-VerifyValue "XLM escrow admin" $deployerAddress @("--id", $escrowXlmId, "--source", $deployerAccount, "--network", "testnet", "--", "get_admin")
    Invoke-VerifyValue "XLM pool asset" $xlmAsset @("--id", $poolXlmId, "--source", $deployerAccount, "--network", "testnet", "--", "get_usdc_asset")
    Invoke-VerifyValue "XLM escrow asset" $xlmAsset @("--id", $escrowXlmId, "--source", $deployerAccount, "--network", "testnet", "--", "get_usdc_asset")
    Invoke-VerifyCheck "pool_xlm_contract (get_stats)" @("--id", $poolXlmId, "--source", $deployerAccount, "--network", "testnet", "--", "get_stats")
}

# 5. USDC Escrow Contract - get_locked (confirm existence with dummy ID)
Invoke-VerifyCheck "escrow_usdc_contract (get_locked)" @("--id", $escrowUsdcId, "--source", $deployerAccount, "--network", "testnet", "--", "get_locked", "--invoice_id", "0000000000000000000000000000000000000000000000000000000000000000")

Invoke-VerifyValue "invoice USDC allow-list" "true" @("--id", $invoiceId, "--source", $deployerAccount, "--network", "testnet", "--", "is_supported_asset", "--asset", $usdcAsset)
if ([Environment]::GetEnvironmentVariable('DEPLOY_XLM') -eq 'true') {
    Invoke-VerifyCheck "escrow_xlm_contract (get_locked)" @("--id", $escrowXlmId, "--source", $deployerAccount, "--network", "testnet", "--", "get_locked", "--invoice_id", "0000000000000000000000000000000000000000000000000000000000000000")
    Invoke-VerifyValue "invoice XLM allow-list" "true" @("--id", $invoiceId, "--source", $deployerAccount, "--network", "testnet", "--", "is_supported_asset", "--asset", $xlmAsset)
}

if ($script:failed) {
    Write-Host "Verification failed."
    exit 1
} else {
    Write-Host "All contract verifications passed successfully."
    exit 0
}
