$ErrorActionPreference = "Continue"

Write-Host "ONDE Setup - Reseau de Resilience Citoyen" -ForegroundColor Green
Write-Host ""

# Step 1: Check Python
Write-Host "[1/4] Checking Python..." -ForegroundColor Yellow
$pythonCmd = Get-Command python -ErrorAction SilentlyContinue
if ($null -eq $pythonCmd) {
    Write-Host "  Installing Python..." -ForegroundColor Red
    winget install --id Python.Python.3.12 -e --accept-source-agreements --accept-package-agreements
    Write-Host "  Please restart PowerShell after Python installation" -ForegroundColor Yellow
    exit 0
} else {
    Write-Host "  Python found" -ForegroundColor Green
}

# Step 2: Install Python packages
Write-Host "[2/4] Installing Python packages (simpy, numpy, etc.)..." -ForegroundColor Yellow
python -m pip install --upgrade pip --user
python -m pip install simpy numpy matplotlib pandas cryptography --user
Write-Host "  Done" -ForegroundColor Green

# Step 3: Init git repo
Write-Host "[3/4] Initializing git repository..." -ForegroundColor Yellow
git init
git add -A
git commit -m "Initial commit: ONDE v0.1.0"
Write-Host "  Done" -ForegroundColor Green

# Step 4: Open UI
Write-Host "[4/4] Opening UI in browser..." -ForegroundColor Yellow
$uiPath = Join-Path (Get-Location).Path "ui\src\index.html"
Start-Process $uiPath
Write-Host "  Done" -ForegroundColor Green

Write-Host ""
Write-Host "Setup complete!" -ForegroundColor Green
Write-Host "Run simulation: python simulation\mesh_sim.py" -ForegroundColor Cyan