@echo off
start "LabPulse SSH" powershell.exe -NoProfile -ExecutionPolicy Bypass -STA -WindowStyle Hidden -File "%~dp0LabPulseSSH.ps1"
