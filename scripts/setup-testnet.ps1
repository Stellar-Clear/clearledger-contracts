#!/usr/bin/env pwsh
# setup-testnet.ps1 — Idempotent testnet deployer setup (PowerShell)
#
# Creates and funds the deployer key on the Stellar testnet.
# Safe to run multiple times: if the key already exists, it funds the
# existing account via Friendbot instead of failing.

$ErrorActionPreference = 'Stop'

# Resolve stellar CLI location dynamically
$stellar = Get-Command stellar -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Source
if (-not $stellar) {
    # Fall back to STELLAR_BIN env var
    $stellarBin = [Environment]::GetEnvironmentVariable('STELLAR_BIN')
    if ($stellarBin -and (Test-Path $stellarBin)) {
        $stellar = $stellarBin
    }
    elseif (Test-Path "${env:ProgramFiles(x86)}\Stellar CLI\stellar.exe") {
        $stellar = "${env:ProgramFiles(x86)}\Stellar CLI\stellar.exe"
    }
    else {
        Write-Host "Error: stellar CLI not found on PATH or default Windows path."
        exit 1
    }
}

$friendbotUrl = "https://friendbot.stellar.org"
$keyName = "deployer"

# Check if deployer key already exists
$keysOutput = & $stellar keys ls 2>$null
if ($keysOutput -match "^${keyName}$") {
    Write-Host "Deployer key '$keyName' already exists — funding existing account via Friendbot..."
    $deployerAddress = & $stellar keys address $keyName
    Write-Host "Deployer address: $deployerAddress"

    try {
        $response = Invoke-WebRequest -Uri "${friendbotUrl}?addr=${deployerAddress}" -Method Get -UseBasicParsing -TimeoutSec 30
        $httpStatus = [int]$response.StatusCode
        if ($httpStatus -eq 200) {
            Write-Host "Friendbot funded successfully."
        }
        elseif ($httpStatus -eq 400) {
            Write-Host "Friendbot returned 400 — account likely already funded (this is normal)."
        }
        else {
            Write-Host "Warning: Friendbot returned HTTP $httpStatus. Funding may not have succeeded."
            Write-Host "         You can retry manually: curl '${friendbotUrl}?addr=${deployerAddress}'"
        }
    }
    catch {
        Write-Host "Warning: Friendbot request failed ($($_.Exception.Message)). Funding may not have succeeded."
    }
}
else {
    Write-Host "Creating and funding testnet deployer account..."
    & $stellar keys generate $keyName --network testnet --fund
    $deployerAddress = & $stellar keys address $keyName
    Write-Host "Deployer address: $deployerAddress"
    Write-Host "Account created and funded."
}

Write-Host "`nDone. Wait ~10 seconds for funding to confirm before running deploy.ps1"
