Unicode True
Name "Swagri Agent"
OutFile "${OUTPUT_DIR}\Swagri-Agent-Setup-x64.exe"
InstallDir "$LOCALAPPDATA\Swagri\Agent"
RequestExecutionLevel user

Page directory
Page instfiles
UninstPage uninstConfirm
UninstPage instfiles

Section "Swagri Agent" SecAgent
  SetOutPath "$INSTDIR"
  File "${BUILD_DIR}\swagri-agent.exe"
  File "${BUILD_DIR}\swagri-updater.exe"
  File "${PACKAGE_DIR}\README.txt"
  WriteUninstaller "$INSTDIR\Uninstall.exe"
  CreateDirectory "$SMPROGRAMS\Swagri"
  CreateShortcut "$SMPROGRAMS\Swagri\Swagri Agent.lnk" "$INSTDIR\swagri-agent.exe" "" "$INSTDIR\swagri-agent.exe" 0 SW_SHOWNORMAL "" "Lightweight Swagri agent"
  CreateShortcut "$SMPROGRAMS\Swagri\Uninstall Swagri Agent.lnk" "$INSTDIR\Uninstall.exe"
SectionEnd

Section "Uninstall"
  Delete "$SMPROGRAMS\Swagri\Swagri Agent.lnk"
  Delete "$SMPROGRAMS\Swagri\Uninstall Swagri Agent.lnk"
  Delete "$INSTDIR\swagri-agent.exe"
  Delete "$INSTDIR\swagri-updater.exe"
  Delete "$INSTDIR\swagri-agent.previous.exe"
  Delete "$INSTDIR\swagri-agent.swagri-new"
  Delete "$INSTDIR\README.txt"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir "$INSTDIR"
  RMDir "$SMPROGRAMS\Swagri"
SectionEnd
