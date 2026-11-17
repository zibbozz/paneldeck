; Paneldeck – Setup (pro Benutzer, ohne Administratorrechte)
; Aufruf: makensis -DVERSION=0.15.0 -DEXE=pfad\paneldeck.exe -DOUTFILE=Paneldeck-Setup-0.15.0.exe installer/paneldeck.nsi
Unicode true
!include "MUI2.nsh"
!include "LogicLib.nsh"
!include "Sections.nsh"

!ifndef VERSION
  !define VERSION "0.0.0"
!endif
!ifndef EXE
  !define EXE "target\x86_64-pc-windows-gnu\release\paneldeck.exe"
!endif
!ifndef OUTFILE
  !define OUTFILE "Paneldeck-Setup-${VERSION}.exe"
!endif
; Mitgelieferte ConPTY (conpty.dll + OpenConsole.exe, geprüft von tools/fetch-conpty.ps1)
!ifndef CONPTY_DIR
  !define CONPTY_DIR "..\dist\conpty"
!endif

; Programmdatei ersetzen, auch wenn Paneldeck läuft: die laufende paneldeck.exe, eine geladene conpty.dll bzw. laufende
; OpenConsole.exe (je Terminal eine) lassen sich nicht überschreiben, aber umbenennen. Die bisherige wird zu NAME.old;
; ist eine ältere NAME.old noch in Benutzung, wird sie unter einem freien Namen NAME.old.<n> beiseitegelegt. Scheitert das
; Schreiben, kommt die bisherige Datei zurück an ihren Platz (nie eine halbe Installation). Der neue Hub löscht alle
; NAME.old* nach seinem ersten Frame; was dann noch läuft, beim nächsten Start (/REBOOTOK nur als Zusatz).
!macro InstallBusyFile SRC NAME
  Delete "$INSTDIR\${NAME}.old"
  ${If} ${FileExists} "$INSTDIR\${NAME}.old"
    StrCpy $R8 1
    ${DoWhile} ${FileExists} "$INSTDIR\${NAME}.old.$R8"
      IntOp $R8 $R8 + 1
    ${Loop}
    Rename "$INSTDIR\${NAME}.old" "$INSTDIR\${NAME}.old.$R8"
    Delete /REBOOTOK "$INSTDIR\${NAME}.old.$R8"
  ${EndIf}
  ${If} ${FileExists} "$INSTDIR\${NAME}"
    Rename "$INSTDIR\${NAME}" "$INSTDIR\${NAME}.old"
  ${EndIf}
  ClearErrors
  File "/oname=${NAME}" "${SRC}"
  ${If} ${Errors}
    Delete "$INSTDIR\${NAME}"
    Rename "$INSTDIR\${NAME}.old" "$INSTDIR\${NAME}"
    MessageBox MB_OK|MB_ICONSTOP "${NAME} konnte nicht geschrieben werden. / Could not write ${NAME}." /SD IDOK
    Abort
  ${EndIf}
  Delete /REBOOTOK "$INSTDIR\${NAME}.old"
!macroend

Name "Paneldeck ${VERSION}"
OutFile "${OUTFILE}"
InstallDir "$LOCALAPPDATA\Programs\Paneldeck"
InstallDirRegKey HKCU "Software\Paneldeck" "InstallDir"
RequestExecutionLevel user
SetCompressor /SOLID lzma
BrandingText "Paneldeck ${VERSION}"

!define MUI_ICON "..\assets\icon.ico"
!define MUI_UNICON "..\assets\icon.ico"
!define MUI_ABORTWARNING
!define MUI_FINISHPAGE_RUN "$INSTDIR\paneldeck.exe"
!define MUI_FINISHPAGE_RUN_TEXT "$(STR_RunNow)"
; Sprachauswahl beim Start (Englisch = Vorgabe/Fallback, Deutsch wird auf deutschen Systemen vorausgewählt); die Wahl merkt sich
; der Deinstaller. Im stillen Modus (/S) erscheint kein Dialog.
!define MUI_LANGDLL_ALWAYSSHOW
!define MUI_LANGDLL_REGISTRY_ROOT HKCU
!define MUI_LANGDLL_REGISTRY_KEY "Software\Paneldeck"
!define MUI_LANGDLL_REGISTRY_VALUENAME "InstallerLanguage"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_LICENSE "..\LICENSE"
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"
!insertmacro MUI_LANGUAGE "German"

LangString STR_RunNow ${LANG_ENGLISH} "Start Paneldeck now"
LangString STR_RunNow ${LANG_GERMAN} "Paneldeck jetzt starten"
LangString NAME_Main ${LANG_ENGLISH} "Paneldeck (required)"
LangString NAME_Main ${LANG_GERMAN} "Paneldeck (erforderlich)"
LangString NAME_Desktop ${LANG_ENGLISH} "Desktop shortcut"
LangString NAME_Desktop ${LANG_GERMAN} "Desktop-Verknüpfung"
LangString NAME_Auto ${LANG_ENGLISH} "Start with Windows (minimized in the tray)"
LangString NAME_Auto ${LANG_GERMAN} "Mit Windows starten (minimiert im Tray)"
LangString NAME_Ctx ${LANG_ENGLISH} "Explorer context menu 'Open in Paneldeck'"
LangString NAME_Ctx ${LANG_GERMAN} "Explorer-Kontextmenü „In Paneldeck öffnen“"
LangString STR_CtxOpen ${LANG_ENGLISH} "Open in Paneldeck"
LangString STR_CtxOpen ${LANG_GERMAN} "In Paneldeck öffnen"
LangString STR_CtxClaude ${LANG_ENGLISH} "Start with Claude in Paneldeck"
LangString STR_CtxClaude ${LANG_GERMAN} "Mit Claude in Paneldeck starten"
LangString STR_Uninst ${LANG_ENGLISH} "Uninstall Paneldeck"
LangString STR_Uninst ${LANG_GERMAN} "Paneldeck deinstallieren"

Function un.onInit
  !insertmacro MUI_UNGETLANGUAGE
FunctionEnd

Section "$(NAME_Main)" SecMain
  SectionIn RO
  SetOutPath "$INSTDIR"
  ; Läuft Paneldeck gerade (Update), sind die Dateien gesperrt – umbenennen darf Windows aber (InstallBusyFile). Die laufende
  ; Instanz bleibt unberührt, die neuen Dateien liegen sofort am Platz. Reihenfolge: erst die ConPTY, dann paneldeck.exe –
  ; bricht die .exe ab, läuft die bisherige mit der neuen ConPTY (verträglich); nie eine neue .exe ohne ihre ConPTY.
  !insertmacro InstallBusyFile "${CONPTY_DIR}\conpty.dll" "conpty.dll"
  !insertmacro InstallBusyFile "${CONPTY_DIR}\OpenConsole.exe" "OpenConsole.exe"
  !insertmacro InstallBusyFile "${EXE}" "paneldeck.exe"
  File "..\README.md"
  File "..\README.de.md"
  File "..\Changelog.md"
  File "..\Changelog.de.md"
  File "..\THIRD_PARTY.md"
  File "..\THIRD_PARTY_LICENSES.txt"
  File "..\LICENSE"
  SetOutPath "$INSTDIR\licenses"
  File "..\assets\fonts\LICENSE-Cascadia.txt"
  File "..\assets\fonts\LICENSE-NotoColorEmoji.txt"
  File "..\assets\fonts\LICENSE-Lucide.txt"
  File "/oname=LICENSE-ConPTY.txt" "..\tools\license-extra\conpty-MIT.txt"
  File "/oname=NOTICE-ConPTY.md" "..\tools\license-extra\conpty-NOTICE.md"
  SetOutPath "$INSTDIR"
  WriteUninstaller "$INSTDIR\Uninstall.exe"

  CreateDirectory "$SMPROGRAMS\Paneldeck"
  CreateShortcut "$SMPROGRAMS\Paneldeck\Paneldeck.lnk" "$INSTDIR\paneldeck.exe"
  CreateShortcut "$SMPROGRAMS\Paneldeck\$(STR_Uninst).lnk" "$INSTDIR\Uninstall.exe"

  WriteRegStr HKCU "Software\Paneldeck" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Paneldeck" "DisplayName" "Paneldeck"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Paneldeck" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Paneldeck" "Publisher" "Paneldeck"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Paneldeck" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Paneldeck" "DisplayIcon" "$INSTDIR\paneldeck.exe"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Paneldeck" "UninstallString" '"$INSTDIR\Uninstall.exe"'
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Paneldeck" "NoModify" 1
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Paneldeck" "NoRepair" 1
SectionEnd

Section "$(NAME_Desktop)" SecDesktop
  CreateShortcut "$DESKTOP\Paneldeck.lnk" "$INSTDIR\paneldeck.exe"
SectionEnd

Section /o "$(NAME_Auto)" SecAutostart
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "Paneldeck" '"$INSTDIR\paneldeck.exe" --tray'
SectionEnd

Section "$(NAME_Ctx)" SecContext
  WriteRegStr HKCU "Software\Classes\Directory\shell\Paneldeck" "" "$(STR_CtxOpen)"
  WriteRegStr HKCU "Software\Classes\Directory\shell\Paneldeck" "Icon" '"$INSTDIR\paneldeck.exe"'
  WriteRegStr HKCU "Software\Classes\Directory\shell\Paneldeck\command" "" '"$INSTDIR\paneldeck.exe" --open "%1"'
  WriteRegStr HKCU "Software\Classes\Directory\Background\shell\Paneldeck" "" "$(STR_CtxOpen)"
  WriteRegStr HKCU "Software\Classes\Directory\Background\shell\Paneldeck" "Icon" '"$INSTDIR\paneldeck.exe"'
  WriteRegStr HKCU "Software\Classes\Directory\Background\shell\Paneldeck\command" "" '"$INSTDIR\paneldeck.exe" --open "%V"'
  WriteRegStr HKCU "Software\Classes\Directory\shell\PaneldeckClaude" "" "$(STR_CtxClaude)"
  WriteRegStr HKCU "Software\Classes\Directory\shell\PaneldeckClaude" "Icon" '"$INSTDIR\paneldeck.exe"'
  WriteRegStr HKCU "Software\Classes\Directory\shell\PaneldeckClaude\command" "" '"$INSTDIR\paneldeck.exe" --claude --open "%1"'
  WriteRegStr HKCU "Software\Classes\Directory\Background\shell\PaneldeckClaude" "" "$(STR_CtxClaude)"
  WriteRegStr HKCU "Software\Classes\Directory\Background\shell\PaneldeckClaude" "Icon" '"$INSTDIR\paneldeck.exe"'
  WriteRegStr HKCU "Software\Classes\Directory\Background\shell\PaneldeckClaude\command" "" '"$INSTDIR\paneldeck.exe" --claude --open "%V"'
SectionEnd

LangString DESC_Main ${LANG_GERMAN} "Das Programm selbst."
LangString DESC_Main ${LANG_ENGLISH} "The program itself."
LangString DESC_Desktop ${LANG_GERMAN} "Verknüpfung auf dem Desktop."
LangString DESC_Desktop ${LANG_ENGLISH} "Shortcut on the desktop."
LangString DESC_Auto ${LANG_GERMAN} "Paneldeck startet mit Windows minimiert im Tray."
LangString DESC_Auto ${LANG_ENGLISH} "Paneldeck starts with Windows, minimized in the tray."
LangString DESC_Ctx ${LANG_GERMAN} "Rechtsklick auf Ordner: In Paneldeck öffnen / Mit Claude starten."
LangString DESC_Ctx ${LANG_ENGLISH} "Right-click a folder: Open in Paneldeck / Start with Claude."
!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN
  !insertmacro MUI_DESCRIPTION_TEXT ${SecMain} $(DESC_Main)
  !insertmacro MUI_DESCRIPTION_TEXT ${SecDesktop} $(DESC_Desktop)
  !insertmacro MUI_DESCRIPTION_TEXT ${SecAutostart} $(DESC_Auto)
  !insertmacro MUI_DESCRIPTION_TEXT ${SecContext} $(DESC_Ctx)
!insertmacro MUI_FUNCTION_DESCRIPTION_END

; Steht hinter den Sections, damit ${SecDesktop}/${SecContext} bekannt sind.
Function .onInit
  !insertmacro MUI_LANGDLL_DISPLAY
  ; Stilles Update über eine bestehende Installation (Selbst-Update aus Paneldeck: /S /D=<ordner>): die bisherige Auswahl
  ; beibehalten, statt die Standardauswahl erneut anzuwenden – eine gelöschte Desktop-Verknüpfung oder ein abgeschaltetes
  ; Kontextmenü kommen nicht zurück. Autostart ist ohnehin abgewählt und bleibt, wie er ist.
  ${If} ${Silent}
  ${AndIf} ${FileExists} "$INSTDIR\paneldeck.exe"
    ${IfNot} ${FileExists} "$DESKTOP\Paneldeck.lnk"
      !insertmacro UnselectSection ${SecDesktop}
    ${EndIf}
    ClearErrors
    ReadRegStr $0 HKCU "Software\Classes\Directory\shell\Paneldeck\command" ""
    ${If} ${Errors}
      !insertmacro UnselectSection ${SecContext}
    ${EndIf}
  ${EndIf}
FunctionEnd

Section "Uninstall"
  ; Globale Claude-Hooks entfernen, solange die .exe noch da ist
  ExecWait '"$INSTDIR\paneldeck.exe" --uninstall-hooks'
  ; Noch laufende/geladene Dateien (Paneldeck oder eine OpenConsole.exe läuft) werden per /REBOOTOK vorgemerkt.
  Delete /REBOOTOK "$INSTDIR\paneldeck.exe"
  Delete /REBOOTOK "$INSTDIR\paneldeck.exe.old*"
  Delete /REBOOTOK "$INSTDIR\conpty.dll"
  Delete /REBOOTOK "$INSTDIR\conpty.dll.old*"
  Delete /REBOOTOK "$INSTDIR\OpenConsole.exe"
  Delete /REBOOTOK "$INSTDIR\OpenConsole.exe.old*"
  Delete "$INSTDIR\README.md"
  Delete "$INSTDIR\README.de.md"
  Delete "$INSTDIR\Changelog.md"
  Delete "$INSTDIR\Changelog.de.md"
  Delete "$INSTDIR\THIRD_PARTY.md"
  Delete "$INSTDIR\THIRD_PARTY_LICENSES.txt"
  Delete "$INSTDIR\LICENSE"
  Delete "$INSTDIR\licenses\LICENSE-Cascadia.txt"
  Delete "$INSTDIR\licenses\LICENSE-NotoColorEmoji.txt"
  Delete "$INSTDIR\licenses\LICENSE-Lucide.txt"
  Delete "$INSTDIR\licenses\LICENSE-ConPTY.txt"
  Delete "$INSTDIR\licenses\NOTICE-ConPTY.md"
  RMDir "$INSTDIR\licenses"
  Delete "$INSTDIR\Uninstall.exe"
  Delete "$SMPROGRAMS\Paneldeck\Paneldeck.lnk"
  Delete "$SMPROGRAMS\Paneldeck\Uninstall Paneldeck.lnk"
  Delete "$SMPROGRAMS\Paneldeck\Paneldeck deinstallieren.lnk"
  RMDir "$SMPROGRAMS\Paneldeck"
  Delete "$DESKTOP\Paneldeck.lnk"
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "Paneldeck"
  DeleteRegKey HKCU "Software\Classes\Directory\shell\Paneldeck"
  DeleteRegKey HKCU "Software\Classes\Directory\shell\PaneldeckClaude"
  DeleteRegKey HKCU "Software\Classes\Directory\Background\shell\Paneldeck"
  DeleteRegKey HKCU "Software\Classes\Directory\Background\shell\PaneldeckClaude"
  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Paneldeck"
  DeleteRegKey HKCU "Software\Paneldeck"
  ; Daten (Einstellungen, Verläufe) nur auf Wunsch löschen
  IfFileExists "$INSTDIR\paneldeck-data\*.*" 0 +3
    MessageBox MB_YESNO|MB_ICONQUESTION "Einstellungen, Sitzungen und gespeicherte Verläufe (paneldeck-data) ebenfalls löschen?$\n$\nDelete settings, sessions and saved history (paneldeck-data) as well?" IDNO +2
    RMDir /r "$INSTDIR\paneldeck-data"
  RMDir "$INSTDIR"
SectionEnd
