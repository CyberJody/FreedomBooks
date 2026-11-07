; Standalone fallback build recipe. Canonical releases use builder.cjs's NSIS
; target. This script requires a complete hardened Windows app and refuses the
; developer shell with a missing native module. Never lower this requirement.
Unicode true
!include "MUI2.nsh"
!include "LogicLib.nsh"
!include "x64.nsh"
!ifndef APP_SOURCE
  !define APP_SOURCE "..\..\electron\out\FreedomBooks-win32-x64"
!endif
!ifndef VERSION
  !define VERSION "0.1.0-d1"
!endif
!if /FileExists "${APP_SOURCE}\resources\app.asar.unpacked\native\fb-node.node"
!else
  !error "Windows native fb-node.node is missing. Do not distribute the incomplete shell."
!endif
Name "FreedomBooks"
OutFile "FreedomBooks-Setup-${VERSION}.exe"
RequestExecutionLevel user
InstallDir "$LOCALAPPDATA\Programs\FreedomBooks"
SilentInstall normal
ShowInstDetails nevershow
!define MUI_ICON "FreedomBooks.ico"
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

Function .onInit
  ${IfNot} ${RunningX64}
    MessageBox MB_ICONSTOP "FreedomBooks needs 64-bit Windows."
    Abort
  ${EndIf}
  SetShellVarContext current
FunctionEnd

Section "Install"
  SetOutPath "$INSTDIR"
  File /r "${APP_SOURCE}\*"
  CreateShortcut "$DESKTOP\FreedomBooks.lnk" "$INSTDIR\FreedomBooks.exe"
  CreateDirectory "$SMPROGRAMS\FreedomBooks"
  CreateShortcut "$SMPROGRAMS\FreedomBooks\FreedomBooks.lnk" "$INSTDIR\FreedomBooks.exe"
  ; electron-builder's NSIS resource bundle supplies WinShell for shortcut AUMI.
  WinShell::SetLnkAUMI "$DESKTOP\FreedomBooks.lnk" "app.freedombooks.FreedomBooks"
  WinShell::SetLnkAUMI "$SMPROGRAMS\FreedomBooks\FreedomBooks.lnk" "app.freedombooks.FreedomBooks"
  WriteUninstaller "$INSTDIR\Uninstall FreedomBooks.exe"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\FreedomBooks" "DisplayName" "FreedomBooks"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\FreedomBooks" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\FreedomBooks" "Publisher" "FreedomBooks"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\FreedomBooks" "DisplayIcon" "$INSTDIR\FreedomBooks.exe"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\FreedomBooks" "UninstallString" '$\"$INSTDIR\Uninstall FreedomBooks.exe$\"'
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\FreedomBooks" "NoModify" 1
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\FreedomBooks" "NoRepair" 1
  Exec '"$INSTDIR\FreedomBooks.exe"'
SectionEnd

Section "Uninstall"
  SetShellVarContext current
  Delete "$DESKTOP\FreedomBooks.lnk"
  Delete "$SMPROGRAMS\FreedomBooks\FreedomBooks.lnk"
  RMDir "$SMPROGRAMS\FreedomBooks"
  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\FreedomBooks"
  ; Only the app install tree is removed; never the user's appData/book folder.
  RMDir /r "$INSTDIR"
SectionEnd
