# Old name of tools\docs-shots.ps1 (now creates all README and guide images); passes every argument on.
#   powershell -ExecutionPolicy Bypass -File tools\readme-shots.ps1 [-Only app,todo-*] [-Lang de] [-NoBuild] ...
& "$PSScriptRoot\docs-shots.ps1" @args
exit $LASTEXITCODE
