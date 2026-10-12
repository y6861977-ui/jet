; NSIS GUI installer for jet (wizard: Welcome -> Dir -> Install -> Finish).
; Installs jet.exe into %LOCALAPPDATA%\Programs\jet (no admin rights),
; adds the folder to the user PATH, and creates an uninstaller.
;
; Build (portable makensis, no NSIS install needed):
;   makensis.exe /DAPPVER=0.1.0 jet-setup.nsi   (jet.exe must sit next to it)
;   -> jet-setup.exe

Unicode true
!include "MUI2.nsh"
!include "LogicLib.nsh"
!include "WinMessages.nsh"
!include "StrFunc.nsh"

; Activate the StrFunc string functions we use
${StrStr}
${UnStrRep}

!ifndef APPVER
  !define APPVER "0.1.0"
!endif
!define UNINST_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\jet"

Name "jet ${APPVER}"
OutFile "jet-setup.exe"
RequestExecutionLevel user
InstallDir "$LOCALAPPDATA\Programs\jet"
InstallDirRegKey HKCU "Software\jet" "InstallDir"
SetCompressor /SOLID lzma

!define MUI_ABORTWARNING

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!define MUI_FINISHPAGE_TEXT "jet has been installed and added to your PATH.$\r$\n$\r$\nOpen a NEW terminal (PowerShell/cmd) and run:  jet help"
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "English"

Section "jet" SecMain
  SetOutPath "$INSTDIR"
  File "jet.exe"
  WriteRegStr HKCU "Software\jet" "InstallDir" "$INSTDIR"

  ; --- Add to the user PATH (no duplicates) ---
  ReadRegStr $0 HKCU "Environment" "Path"
  ${StrStr} $1 ";$0;" ";$INSTDIR;"
  ${If} $1 == ""
    ${If} $0 == ""
      WriteRegExpandStr HKCU "Environment" "Path" "$INSTDIR"
    ${Else}
      WriteRegExpandStr HKCU "Environment" "Path" "$0;$INSTDIR"
    ${EndIf}
    SendMessage ${HWND_BROADCAST} ${WM_WININICHANGE} 0 "STR:Environment" /TIMEOUT=5000
  ${EndIf}

  ; --- Register in "Apps & features" / "Add or remove programs" ---
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayName" "jet ${APPVER}"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayVersion" "${APPVER}"
  WriteRegStr HKCU "${UNINST_KEY}" "Publisher" "jet"
  WriteRegStr HKCU "${UNINST_KEY}" "URLInfoAbout" "https://jet.kim"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayIcon" "$INSTDIR\jet.exe"
  WriteRegStr HKCU "${UNINST_KEY}" "UninstallString" "$INSTDIR\uninstall.exe"
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoRepair" 1
  WriteUninstaller "$INSTDIR\uninstall.exe"
SectionEnd

Section "Uninstall"
  Delete "$INSTDIR\jet.exe"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"
  DeleteRegKey HKCU "Software\jet"
  DeleteRegKey HKCU "${UNINST_KEY}"

  ; --- Remove from PATH ---
  ReadRegStr $0 HKCU "Environment" "Path"
  ${UnStrRep} $0 "$0" ";$INSTDIR" ""
  ${UnStrRep} $0 "$0" "$INSTDIR;" ""
  ${UnStrRep} $0 "$0" "$INSTDIR" ""
  WriteRegExpandStr HKCU "Environment" "Path" "$0"
  SendMessage ${HWND_BROADCAST} ${WM_WININICHANGE} 0 "STR:Environment" /TIMEOUT=5000
SectionEnd
