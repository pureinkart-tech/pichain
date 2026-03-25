@echo off
REM PIChain Miner — Windows Installer
REM Double-click this file to set up the miner automatically.

echo.
echo   ======================================================
echo   =         PIChain Miner — Windows Setup               =
echo   ======================================================
echo.

cd /d "%~dp0"

REM Check if miner exists
if not exist "pichain-miner-windows-x86_64.exe" (
    echo   Downloading miner...
    powershell -Command "Invoke-WebRequest -Uri 'https://github.com/pureinkart-tech/pichain/releases/latest/download/pichain-miner-windows-x86_64.exe' -OutFile 'pichain-miner-windows-x86_64.exe'"
)

REM Check if signer exists
if not exist "pichain-signer-windows-x86_64.exe" (
    echo   Downloading signer...
    powershell -Command "Invoke-WebRequest -Uri 'https://github.com/pureinkart-tech/pichain/releases/latest/download/pichain-signer-windows-x86_64.exe' -OutFile 'pichain-signer-windows-x86_64.exe'"
)

REM Generate wallet if needed
if not exist "wallet.json" (
    echo.
    echo   Creating your quantum-safe wallet...
    pichain-miner-windows-x86_64.exe --keypair wallet.json --generate-keypair
)

echo.
echo   Setup complete!
echo.
echo   Your wallet: wallet.json
echo.
echo   To start mining:
echo     pichain-miner-windows-x86_64.exe --keypair wallet.json
echo.
echo   To use DEX/staking from browser:
echo     pichain-signer-windows-x86_64.exe --wallet wallet.json
echo     Then open https://pichain.net
echo.

set /p choice="  Start mining now? (y/n) "
if /i "%choice%"=="y" (
    echo.
    pichain-miner-windows-x86_64.exe --keypair wallet.json --rpc-url https://pichain.net --profile desktop
)

pause
