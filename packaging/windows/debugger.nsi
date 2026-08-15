Unicode True
Name "Swagri Debugger"
OutFile "${OUTPUT_DIR}\Swagri-Debugger-Setup-x64.exe"
InstallDir "$LOCALAPPDATA\Swagri\Debugger"
RequestExecutionLevel user

Page directory
Page instfiles
UninstPage uninstConfirm
UninstPage instfiles

Section "Swagri Debugger" SecDebugger
  SetOutPath "$INSTDIR"
  File "${BUILD_DIR}\swagri-debugger.exe"
  File "${BUILD_DIR}\swagri-agent.exe"
  File "${PACKAGE_DIR}\README.txt"
  WriteUninstaller "$INSTDIR\Uninstall.exe"
  CreateDirectory "$SMPROGRAMS\Swagri"
  CreateShortcut "$SMPROGRAMS\Swagri\Swagri Debugger.lnk" "$INSTDIR\swagri-debugger.exe" "" "$INSTDIR\swagri-debugger.exe" 0 SW_SHOWNORMAL "" "Swagri desktop debugger"
  CreateShortcut "$DESKTOP\Swagri Debugger.lnk" "$INSTDIR\swagri-debugger.exe"
  CreateShortcut "$SMPROGRAMS\Swagri\Uninstall Swagri Debugger.lnk" "$INSTDIR\Uninstall.exe"
SectionEnd

Section "Uninstall"
  Delete "$DESKTOP\Swagri Debugger.lnk"
  Delete "$SMPROGRAMS\Swagri\Swagri Debugger.lnk"
  Delete "$SMPROGRAMS\Swagri\Uninstall Swagri Debugger.lnk"
  Delete "$INSTDIR\swagri-debugger.exe"
  Delete "$INSTDIR\swagri-agent.exe"
  Delete "$INSTDIR\README.txt"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir "$INSTDIR"
  RMDir "$SMPROGRAMS\Swagri"
SectionEnd
