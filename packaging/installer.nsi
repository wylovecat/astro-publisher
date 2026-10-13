; NSIS installer for astro-publisher.
;
; Per-user install (no UAC): everything lands under
; %LOCALAPPDATA%\Programs\astro-publisher and the uninstall entry is written to
; HKCU, so nothing needs administrator rights.
;
; Build:  makensis /DAPP_VERSION=0.1.0 /DSOURCE_DIR=..\target\release packaging\installer.nsi

Unicode true

!include "MUI2.nsh"
!include "FileFunc.nsh"
!include "LogicLib.nsh"

; --- Inputs (with defaults so the script can be run directly) ---------------
!ifndef APP_VERSION
  !define APP_VERSION "0.1.0"
!endif
!ifndef SOURCE_DIR
  !define SOURCE_DIR "..\target\release"
!endif
!ifndef OUT_FILE
  !define OUT_FILE "..\dist\astro-publisher-${APP_VERSION}-setup.exe"
!endif

!define APP_NAME "astro-publisher"
!define APP_DISPLAY "Astro Publisher"
!define APP_PUBLISHER "wylovecat"
!define APP_URL "https://github.com/wylovecat/astro-publisher"
!define APP_EXE "astro-publisher.exe"
!define APP_CLI "astro-publisher-core.exe"

Name "${APP_DISPLAY} ${APP_VERSION}"
OutFile "${OUT_FILE}"
InstallDir "$LOCALAPPDATA\Programs\${APP_NAME}"
InstallDirRegKey HKCU "Software\${APP_NAME}" "InstallDir"
RequestExecutionLevel user          ; per-user install, never elevate
SetCompressor /SOLID lzma

VIProductVersion "${APP_VERSION}.0"
VIAddVersionKey "ProductName" "${APP_DISPLAY}"
VIAddVersionKey "FileDescription" "${APP_DISPLAY} installer"
VIAddVersionKey "FileVersion" "${APP_VERSION}"
VIAddVersionKey "ProductVersion" "${APP_VERSION}"
VIAddVersionKey "CompanyName" "${APP_PUBLISHER}"
VIAddVersionKey "LegalCopyright" "MIT License"

; --- Pages ------------------------------------------------------------------
!define MUI_ABORTWARNING
!define MUI_FINISHPAGE_RUN "$INSTDIR\${APP_EXE}"
!define MUI_FINISHPAGE_RUN_TEXT "启动 ${APP_DISPLAY}"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "SimpChinese"
!insertmacro MUI_LANGUAGE "English"

; --- Install ----------------------------------------------------------------
Section "Application" SecMain
  SectionIn RO
  SetOutPath "$INSTDIR"

  File "${SOURCE_DIR}\${APP_EXE}"
  ; The CLI is optional in the build: include it when present.
  IfFileExists "${SOURCE_DIR}\${APP_CLI}" 0 +2
    File "${SOURCE_DIR}\${APP_CLI}"

  ; Uninstaller
  WriteUninstaller "$INSTDIR\uninstall.exe"

  ; Start menu entry
  CreateDirectory "$SMPROGRAMS\${APP_DISPLAY}"
  CreateShortcut "$SMPROGRAMS\${APP_DISPLAY}\${APP_DISPLAY}.lnk" "$INSTDIR\${APP_EXE}"
  CreateShortcut "$SMPROGRAMS\${APP_DISPLAY}\卸载 ${APP_DISPLAY}.lnk" "$INSTDIR\uninstall.exe"

  ; Optional desktop shortcut (unchecked by default is not offered; keep it out
  ; of the way — a desktop icon is easy to add by hand if wanted).

  ; Registry: install location + Add/Remove Programs entry (HKCU = per-user)
  WriteRegStr HKCU "Software\${APP_NAME}" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "Software\${APP_NAME}" "Version" "${APP_VERSION}"

  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_NAME}" \
    "DisplayName" "${APP_DISPLAY}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_NAME}" \
    "DisplayVersion" "${APP_VERSION}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_NAME}" \
    "Publisher" "${APP_PUBLISHER}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_NAME}" \
    "DisplayIcon" "$INSTDIR\${APP_EXE}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_NAME}" \
    "URLInfoAbout" "${APP_URL}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_NAME}" \
    "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_NAME}" \
    "QuietUninstallString" '"$INSTDIR\uninstall.exe" /S'
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_NAME}" \
    "NoModify" 1
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_NAME}" \
    "NoRepair" 1

  ; Report the installed size to Add/Remove Programs.
  ${GetSize} "$INSTDIR" "/S=0K" $0 $1 $2
  IntFmt $0 "0x%08X" $0
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_NAME}" \
    "EstimatedSize" "$0"
SectionEnd

; --- Uninstall --------------------------------------------------------------
Section "Uninstall"
  Delete "$INSTDIR\${APP_EXE}"
  Delete "$INSTDIR\${APP_CLI}"
  Delete "$INSTDIR\uninstall.exe"

  Delete "$SMPROGRAMS\${APP_DISPLAY}\${APP_DISPLAY}.lnk"
  Delete "$SMPROGRAMS\${APP_DISPLAY}\卸载 ${APP_DISPLAY}.lnk"
  RMDir "$SMPROGRAMS\${APP_DISPLAY}"

  ; Remove the directory only if the user left no files in it.
  RMDir "$INSTDIR"

  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_NAME}"
  DeleteRegKey HKCU "Software\${APP_NAME}"

  ; The user's own settings under %APPDATA% are intentionally kept, so
  ; reinstalling keeps the configured repository path.
SectionEnd

; --- Helpers ----------------------------------------------------------------
Function .onInit
  ; Refuse to install over a running copy: the files would be locked.
  FindWindow $0 "" "${APP_DISPLAY}"
  StrCmp $0 0 done
    MessageBox MB_OKCANCEL|MB_ICONEXCLAMATION \
      "${APP_DISPLAY} 正在运行，请先关闭它再继续安装。" \
      IDOK done
    Abort
  done:
FunctionEnd
