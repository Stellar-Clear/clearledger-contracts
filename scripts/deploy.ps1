#!/usr/bin/env pwsh
# deploy.ps1 — Idempotent deployment script for ClearLedger contracts (PowerShell)
#
# Usage:
#   powershell ./scripts/deploy.ps1              # Normal deploy (skips already-deployed contracts)
#   powershell ./scripts/deploy.ps1 -Fresh        # Ignore saved addresses and redeploy everything
#   powershell ./scripts/deploy.ps1 -DryRun       # Show what would be deployed without actually deploying
#   powershell ./scripts/deploy.ps1 -Help         # Show this help
#
# Deployed addresses are persisted to .deployed-addresses after each successful
# deployment step.  Re-running the script after a partial failure will skip any
# step whose address was already saved.

$ErrorActionPreference = 'Stop'

# ---------------------------------------------------------------------------
# 0. CLI / env setup
# ---------------------------------------------------------------------------

$fresh = $false
$resume = $false
$dryRun = $false

foreach ($arg in $args) {
    switch -Wildcard ($arg) {
        '-fresh'    { $fresh = $true }
        '-resume'   { $resume = $true }
        '-dryrun'   { $dryRun = $true }
        '-help'     { Write-Host "deploy.ps1 — Idempotent deployment script for ClearLedger contracts"; exit 0 }
        '-h'        { Write-Host "deploy.ps1 — Idempotent deployment script for ClearLedger contracts"; exit 0 }
        default     { Write-Host "Unknown argument: $arg  (use -Help for usage)"; exit 1 }
    }
}

# Resolve stellar CLI location dynamically
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

# Load .env file
$envFile = if (Test-Path .env) { ".env" } else { ".env.example" }
Get-Content $envFile | ForEach-Object {
    $line = $_.Trim()
    if ($line -and -not $line.StartsWith('#') -and $line -match '^([A-Za-z_][A-Za-z0-9_]*)=(.*)$') {
        $key = $matches[1]
        if ($key -in @('DEPLOYER_ACCOUNT', 'DEPLOY_XLM', 'USDC_ISSUER', 'XLM_ASSET', 'AGENT_REGISTRY_CONTRACT', 'STELLAR_BIN')) {
            $value = $matches[2].Trim()
            if ($value.Length -ge 2 -and (($value.StartsWith('"') -and $value.EndsWith('"')) -or ($value.StartsWith("'") -and $value.EndsWith("'")))) {
                $value = $value.Substring(1, $value.Length - 2)
            }
            [Environment]::SetEnvironmentVariable($key, $value)
        }
    }
}
$deployXlm = $env:DEPLOY_XLM -eq 'true'

# ---------------------------------------------------------------------------
# 1. Address persistence helpers
# ---------------------------------------------------------------------------

$addressesFile = ".deployed-addresses"

if ($fresh) {
    Write-Host "=== -Fresh flag set: removing saved addresses and starting clean ==="
    Remove-Item -Path $addressesFile -ErrorAction SilentlyContinue
}

# Ensure file exists
if (-not (Test-Path $addressesFile)) {
    New-Item -Path $addressesFile -ItemType File -Force | Out-Null
}

# Read all saved addresses into a hashtable
function Get-SavedAddresses {
    $map = @{}
    if (Test-Path $addressesFile) {
        Get-Content $addressesFile | ForEach-Object {
            if ($_ -match '^([^=]+)=(.*)$') {
                $map[$matches[1]] = $matches[2]
            }
        }
    }
    return $map
}

function Save-Address {
    param($key, $value)
    $map = Get-SavedAddresses
    $map[$key] = $value
    $map.Keys | ForEach-Object { "$_=$($map[$_])" } | Set-Content $addressesFile
}

function Load-Address {
    param($key)
    $map = Get-SavedAddresses
    return $map[$key]
}

# ---------------------------------------------------------------------------
# 2. Transaction confirmation polling
# ---------------------------------------------------------------------------

function Wait-ForContract {
    param($contractId)
    $maxAttempts = 15
    $attempt = 0
    $delay = 2

    Write-Host "  Waiting for contract $contractId to be confirmed on-chain..."
    while ($attempt -lt $maxAttempts) {
        $wasmPath = Join-Path ([System.IO.Path]::GetTempPath()) "clearledger-$contractId.wasm"
        $output = & $stellar contract fetch --id $contractId --network testnet --out-file $wasmPath 2>&1
        if (Test-Path $wasmPath) { Remove-Item $wasmPath -Force }
        if ($LASTEXITCODE -eq 0) {
            Write-Host "  Confirmed."
            return $true
        }
        $attempt++
        Write-Host "  Attempt $attempt/$maxAttempts — not yet visible, retrying in ${delay}s..."
        Start-Sleep -Seconds $delay
    }

    Write-Host "ERROR: Contract $contractId was not confirmed after $(($maxAttempts * $delay))s."
    Write-Host "       Check your network connection or the Stellar testnet status."
    Write-Host "       The address has NOT been saved.  Re-run the script to resume."
    return $false
}

# ---------------------------------------------------------------------------
# 3. Core deployment helper
# ---------------------------------------------------------------------------

function Deploy-Contract {
    param($key, $wasm)

    $existing = Load-Address $key
    if ($existing) {
        Write-Host "  $key already deployed at $existing — skipping."
        return $existing
    }

    if ($dryRun) {
        Write-Host "  [DRY-RUN] Would deploy $key from $wasm"
        return "<dry-run>"
    }

    if (-not (Test-Path $wasm)) {
        Write-Host "ERROR: WASM file not found: $wasm"
        Write-Host "       Run 'stellar contract build' first, or check the build output."
        throw "WASM not found: $wasm"
    }

    Write-Host "  Deploying $key from $wasm ..."
    $contractId = & $stellar contract deploy --wasm $wasm --source $env:DEPLOYER_ACCOUNT --network testnet 2>&1
    if ($LASTEXITCODE -ne 0) {
        Write-Host "ERROR: Deploy failed for $key:"
        Write-Host "  Stellar CLI output withheld to avoid logging transaction signing material."
        Write-Host "  Fix the error above and rerun — already-deployed contracts will be skipped."
        throw "Deploy failed for $key"
    }

    # Trim whitespace from the contract ID
    $contractId = $contractId.Trim()

    if (-not (Wait-ForContract $contractId)) {
        throw "Contract $contractId was not confirmed"
    }

    Save-Address $key $contractId
    Write-Host "  Saved: $key=$contractId"
    return $contractId
}

# ---------------------------------------------------------------------------
# 4. Initialization helper
# ---------------------------------------------------------------------------

function Invoke-Init {
    param($label, $contractId, [string[]]$initArgs)

    $initKey = "${label}_initialized"
    $existing = Load-Address $initKey
    if ($existing) {
        Write-Host "  $label already initialized — skipping."
        return
    }

    if ($dryRun) {
        Write-Host "  [DRY-RUN] Would initialize $label ($contractId) with: $($initArgs -join ' ')"
        return
    }

    Write-Host "  Initializing $label ($contractId) ..."
    $output = & $stellar contract invoke --id $contractId --source $env:DEPLOYER_ACCOUNT --network testnet @initArgs 2>&1
    if ($LASTEXITCODE -ne 0) {
        Write-Host "ERROR: Initialization failed for $label:"
        Write-Host "  Stellar CLI output withheld to avoid logging transaction signing material."
        Write-Host "  The contract is deployed but NOT initialized."
        Write-Host "  Fix the error and rerun — this step will be retried automatically."
        throw "Init failed for $label"
    }

    Save-Address $initKey "true"
    Write-Host "  $label initialized successfully."
}

# ---------------------------------------------------------------------------
# 5. Pre-flight checks
# ---------------------------------------------------------------------------

Write-Host "`n=== Pre-flight checks ==="

$deployerAccount = [Environment]::GetEnvironmentVariable('DEPLOYER_ACCOUNT')
if (-not $deployerAccount) {
    Write-Host "ERROR: DEPLOYER_ACCOUNT is not set.  Check your .env file."
    exit 1
}

$usdcIssuer = [Environment]::GetEnvironmentVariable('USDC_ISSUER')
if (-not $usdcIssuer) {
    Write-Host "ERROR: USDC_ISSUER is not set.  Check your .env file."
    exit 1
}

$xlmAsset = [Environment]::GetEnvironmentVariable('XLM_ASSET')
if ($deployXlm -and -not $xlmAsset) {
    Write-Host "ERROR: XLM_ASSET is not set.  Check your .env file."
    exit 1
}

$deployerAddress = & $stellar keys address $deployerAccount 2>&1
if ($LASTEXITCODE -ne 0) {
    Write-Host "ERROR: Could not resolve address for key '$deployerAccount'."
    Write-Host "       Run scripts/setup-testnet.ps1 first."
    exit 1
}
Write-Host "  Deployer address : $deployerAddress"
Write-Host "  Addresses file   : $addressesFile"
Write-Host "  Fresh deploy     : $fresh"
Write-Host "  Dry-run mode     : $dryRun"
Write-Host "  Deploy XLM       : $deployXlm"
Write-Host ""

# ---------------------------------------------------------------------------
# 6. Build
# ---------------------------------------------------------------------------

if ($dryRun) {
    Write-Host "=== [DRY-RUN] Skipping build ==="
} else {
    Write-Host "=== Building all contracts ==="
    & $stellar contract build
}
Write-Host ""

# ---------------------------------------------------------------------------
# 7. Deploy & initialize all contracts
# ---------------------------------------------------------------------------

Write-Host "=== Deploying registry_contract ==="
$registryId = Deploy-Contract "registry" "target/wasm32v1-none/release/clearledger_registry.wasm"
Write-Host "Registry: $registryId"

Invoke-Init "registry" $registryId @("--", "initialize", "--admin", $deployerAddress)

Write-Host "`n=== Deploying invoice_contract ==="
$invoiceId = Deploy-Contract "invoice" "target/wasm32v1-none/release/clearledger_invoice.wasm"
Write-Host "Invoice: $invoiceId"

Invoke-Init "invoice" $invoiceId @("--", "initialize", "--admin", $deployerAddress, "--registry_contract", $registryId)

Write-Host "`n=== Deploying USDC escrow_contract ==="
$escrowUsdcId = Deploy-Contract "escrow_usdc" "target/wasm32v1-none/release/clearledger_escrow.wasm"
Write-Host "USDC Escrow: $escrowUsdcId"

Write-Host "`n=== Deploying USDC pool_contract ==="
$poolUsdcId = Deploy-Contract "pool_usdc" "target/wasm32v1-none/release/clearledger_pool.wasm"
Write-Host "USDC Pool: $poolUsdcId"

Invoke-Init "escrow_usdc" $escrowUsdcId @("--", "initialize", "--admin", $deployerAddress, "--pool_contract", $poolUsdcId, "--usdc_asset", $usdcIssuer)

Invoke-Init "pool_usdc" $poolUsdcId @("--", "initialize", "--admin", $deployerAddress, "--invoice_contract", $invoiceId, "--escrow_contract", $escrowUsdcId, "--funding_asset", $usdcIssuer, "--registry_contract", $registryId, "--treasury", $deployerAddress, "--min_initial_deposit", "10000000", "--share_name", "`"ClearLedger USDC Pool Shares`"", "--share_symbol", "`"TT-USDC`"", "--share_decimals", "7")

 $escrowXlmId = ''
 $poolXlmId = ''
if ($deployXlm) {
    Write-Host "`n=== Deploying XLM escrow_contract (EXPERIMENTAL) ==="
    $escrowXlmId = Deploy-Contract "escrow_xlm" "target/wasm32v1-none/release/clearledger_escrow.wasm"
    Write-Host "XLM Escrow: $escrowXlmId"

    Write-Host "`n=== Deploying XLM pool_contract (EXPERIMENTAL) ==="
    $poolXlmId = Deploy-Contract "pool_xlm" "target/wasm32v1-none/release/clearledger_pool.wasm"
    Write-Host "XLM Pool: $poolXlmId"

    Invoke-Init "escrow_xlm" $escrowXlmId @("--", "initialize", "--admin", $deployerAddress, "--pool_contract", $poolXlmId, "--usdc_asset", $xlmAsset)

    Invoke-Init "pool_xlm" $poolXlmId @("--", "initialize", "--admin", $deployerAddress, "--invoice_contract", $invoiceId, "--escrow_contract", $escrowXlmId, "--funding_asset", $xlmAsset, "--registry_contract", $registryId, "--treasury", $deployerAddress, "--min_initial_deposit", "10000000", "--share_name", "`"ClearLedger XLM Pool Shares`"", "--share_symbol", "`"TT-XLM`"", "--share_decimals", "7")
}

Write-Host "`n=== Wiring USDC pool_contract into invoice_contract ==="
Invoke-Init "invoice_set_pool" $invoiceId @("--", "set_pool_contract", "--pool_contract", $poolUsdcId)

# Without the escrow wiring below, repay / repay_partial / repay_early panic
# with InvoiceError::NotFound when reading DataKey::EscrowContract.
Write-Host "`n=== Wiring USDC escrow_contract into invoice_contract ==="
Invoke-Init "invoice_set_escrow" $invoiceId @("--", "set_escrow_contract", "--escrow_contract", $escrowUsdcId)

# Without allow-listing, create rejects every invoice with UnsupportedAsset.
# add_supported_asset is itself idempotent on-chain (a no-op for an asset
# that is already allow-listed).
Write-Host "`n=== Allow-listing USDC as a supported funding asset ==="
Invoke-Init "invoice_add_asset_usdc" $invoiceId @("--", "add_supported_asset", "--asset", $usdcIssuer)

if ($deployXlm) {
    Write-Host "`n=== Allow-listing XLM as a supported funding asset (EXPERIMENTAL) ==="
    Invoke-Init "invoice_add_asset_xlm" $invoiceId @("--", "add_supported_asset", "--asset", $xlmAsset)
}

# Agent-registry wiring is optional: it is only performed when
# AGENT_REGISTRY_CONTRACT is set in .env / .env.example.  Without it,
# submit_attestation panics with InvoiceError::NotFound and
# list_for_financing can never unlock.
$agentRegistryContract = [Environment]::GetEnvironmentVariable('AGENT_REGISTRY_CONTRACT')
if ($agentRegistryContract) {
    Write-Host "`n=== Wiring agent_registry_contract into invoice_contract ==="
    Invoke-Init "invoice_set_agent_registry" $invoiceId @("--", "set_agent_registry_contract", "--agent_registry_contract", $agentRegistryContract)
} else {
    Write-Host "`n=== AGENT_REGISTRY_CONTRACT is not set — skipping agent-registry wiring ==="
    Write-Host "    submit_attestation will fail until it is wired (see DEPLOYMENT.md)."
}

# ---------------------------------------------------------------------------
# 8. Persist final addresses
# ---------------------------------------------------------------------------

if ($dryRun) {
    Write-Host "=== [DRY-RUN] Skipping .env.deployed generation ==="
} else {
$envOut = ".env.deployed"
$timestamp = (Get-Date -Format 'yyyy-MM-ddTHH:mm:ssZ')
@"
# Generated by deploy.ps1 on $timestamp
# Copy these values into clearledger-app .env.local

NEXT_PUBLIC_REGISTRY_CONTRACT_ID=$registryId
NEXT_PUBLIC_INVOICE_CONTRACT_ID=$invoiceId
NEXT_PUBLIC_ESCROW_USDC_CONTRACT_ID=$escrowUsdcId
NEXT_PUBLIC_ESCROW_XLM_CONTRACT_ID=$escrowXlmId # EXPERIMENTAL
NEXT_PUBLIC_POOL_USDC_CONTRACT_ID=$poolUsdcId
NEXT_PUBLIC_POOL_XLM_CONTRACT_ID=$poolXlmId # EXPERIMENTAL
"@ | Set-Content $envOut

$deploymentMetadata = [ordered]@{
    registry = $registryId
    invoice = $invoiceId
    escrow_usdc = $escrowUsdcId
    pool_usdc = $poolUsdcId
    escrow_xlm = $escrowXlmId
    pool_xlm = $poolXlmId
    updated_at = [DateTime]::UtcNow.ToString('yyyy-MM-ddTHH:mm:ssZ')
}
$deploymentMetadata | ConvertTo-Json | Set-Content -Encoding utf8 deployments.json

$readmePath = Join-Path (Get-Location) 'README.md'
if (Test-Path $readmePath) {
    $readme = [System.IO.File]::ReadAllText($readmePath)
    $startMarker = '<!-- START_DEPLOYED_ADDRESSES -->'
    $endMarker = '<!-- END_DEPLOYED_ADDRESSES -->'
    $startIndex = $readme.IndexOf($startMarker, [System.StringComparison]::Ordinal)
    $endIndex = $readme.IndexOf($endMarker, [System.StringComparison]::Ordinal)
    if ($startIndex -lt 0 -or $endIndex -le $startIndex) {
        throw 'README.md deployment address markers are missing or out of order.'
    }

    $rows = @(
        '| Contract | Address | Stellar Expert |'
        '|----------|---------|---------------|'
    )
    foreach ($entry in @(
        @{ Name = 'registry_contract'; Id = $registryId }
        @{ Name = 'invoice_contract'; Id = $invoiceId }
        @{ Name = 'escrow_contract'; Id = $escrowUsdcId }
        @{ Name = 'pool_contract'; Id = $poolUsdcId }
        @{ Name = 'escrow_xlm_contract (experimental)'; Id = $escrowXlmId }
        @{ Name = 'pool_xlm_contract (experimental)'; Id = $poolXlmId }
    )) {
        if ($entry.Id) {
            $url = "https://stellar.expert/explorer/testnet/contract/$($entry.Id)"
            $rows += "| $($entry.Name) | ``$($entry.Id)`` | [View]($url) |"
        }
    }

    $prefix = $readme.Substring(0, $startIndex + $startMarker.Length)
    $suffix = $readme.Substring($endIndex)
    $updatedReadme = "$prefix`n$([string]::Join("`n", $rows))`n$suffix"
    [System.IO.File]::WriteAllText($readmePath, $updatedReadme, [System.Text.UTF8Encoding]::new($false))
}

Write-Host "`n==========================================="
Write-Host "Deployment complete."
Write-Host ""
Write-Host "Addresses saved to: $addressesFile"
Write-Host "Deployment metadata saved to: deployments.json"
Write-Host "Frontend env saved to: $envOut"
Write-Host ""
Write-Host "Add to clearledger-app .env.local:"
Write-Host ""
Get-Content $envOut | ForEach-Object { Write-Host $_ }
Write-Host "==========================================="
}
