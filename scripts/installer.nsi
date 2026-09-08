Unicode true
ManifestDPIAware true
RequestExecutionLevel user
SetCompressor /SOLID lzma
SetDatablockOptimize on

!ifndef PRODUCT_NAME
  !define PRODUCT_NAME "RemoteDeck"
!endif
!ifndef APP_VERSION
  !define APP_VERSION "2.1.0"
!endif
!ifndef APP_BINARY
  !error "APP_BINARY must point to the release RemoteDeck.exe"
!endif
!ifndef APP_ICON
  !error "APP_ICON must point to the RemoteDeck installer icon"
!endif
!ifndef OUTFILE
  !define OUTFILE "RemoteDeck-${APP_VERSION}-win-x64-setup.exe"
!endif

Name "${PRODUCT_NAME}"
Caption "${PRODUCT_NAME} ${APP_VERSION}"
OutFile "${OUTFILE}"
Icon "${APP_ICON}"
InstallDir "$LOCALAPPDATA\Programs\RemoteDeck"
InstallDirRegKey HKCU "Software\RemoteDeck" "InstallDir"
BrandingText "${PRODUCT_NAME}"
VIProductVersion "${APP_VERSION}.0"
VIAddVersionKey ProductName "${PRODUCT_NAME}"
VIAddVersionKey ProductVersion "${APP_VERSION}"
VIAddVersionKey FileVersion "${APP_VERSION}"
VIAddVersionKey FileDescription "RemoteDeck local SSH service and WebUI"
VIAddVersionKey LegalCopyright "MIT License"

!include "MUI2.nsh"
!include "FileFunc.nsh"

!define MUI_ABORTWARNING
!define MUI_ICON "${APP_ICON}"
!define MUI_UNICON "${APP_ICON}"
!define MUI_WELCOMEPAGE_TEXT "${PRODUCT_NAME} runs the local service and opens the browser WebUI."
!define MUI_FINISHPAGE_RUN "$INSTDIR\RemoteDeck.exe"
!define MUI_FINISHPAGE_RUN_TEXT "Start RemoteDeck"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

Function .onInit
  SetShellVarContext current
FunctionEnd

Function un.onInit
  SetShellVarContext current
FunctionEnd

Section "RemoteDeck" SEC_MAIN
  SetShellVarContext current
  SetOutPath "$INSTDIR"
  File "${APP_BINARY}"
  WriteUninstaller "$INSTDIR\uninstall.exe"

  WriteRegStr HKCU "Software\RemoteDeck" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\RemoteDeck" "DisplayName" "${PRODUCT_NAME}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\RemoteDeck" "DisplayVersion" "${APP_VERSION}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\RemoteDeck" "Publisher" "RemoteDeck contributors"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\RemoteDeck" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\RemoteDeck" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\RemoteDeck" "NoModify" 1
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\RemoteDeck" "NoRepair" 1

  CreateDirectory "$SMPROGRAMS\RemoteDeck"
  CreateShortCut "$SMPROGRAMS\RemoteDeck\RemoteDeck.lnk" "$INSTDIR\RemoteDeck.exe"
  CreateShortCut "$DESKTOP\RemoteDeck.lnk" "$INSTDIR\RemoteDeck.exe"
SectionEnd

Section "Uninstall"
  SetShellVarContext current
  ; Never terminate arbitrary processes. The service must be stopped by the user
  ; before uninstall; NSIS then removes only files belonging to this install.
  Delete "$DESKTOP\RemoteDeck.lnk"
  Delete "$SMPROGRAMS\RemoteDeck\RemoteDeck.lnk"
  RMDir "$SMPROGRAMS\RemoteDeck"
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "RemoteDeck"
  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\RemoteDeck"
  DeleteRegKey HKCU "Software\RemoteDeck"
  Delete "$INSTDIR\uninstall.exe"
  Delete "$INSTDIR\RemoteDeck.exe"
  RMDir "$INSTDIR"
  ; App data is intentionally outside $INSTDIR and survives uninstall.
SectionEnd
