; Paneldeck – Setup (pro Benutzer, ohne Administratorrechte)
; Aufruf: makensis -DVERSION=0.15.0 -DEXE=pfad\paneldeck.exe -DOUTFILE=Paneldeck-Setup-0.15.0.exe installer/paneldeck.nsi
Unicode true
!include "MUI2.nsh"
!include "LogicLib.nsh"

!ifndef VERSION
  !define VERSION "0.0.0"
!endif
!ifndef EXE
  !define EXE "target\x86_64-pc-windows-gnu\release\paneldeck.exe"
!endif
!ifndef OUTFILE
  !define OUTFILE "Paneldeck-Setup-${VERSION}.exe"
!endif

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
!define MUI_FINISHPAGE_RUN_TEXT "Paneldeck jetzt starten / Start Paneldeck now"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_LICENSE "..\LICENSE"
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "German"
!insertmacro MUI_LANGUAGE "English"

Section "Paneldeck (erforderlich)" SecMain
  SectionIn RO
  SetOutPath "$INSTDIR"
  ; Läuft Paneldeck gerade (Update), ist die .exe gesperrt – umbenennen darf Windows aber. Die laufende Instanz
  ; bleibt unberührt (alte Datei bis zum Beenden), die neue liegt sofort am Platz; der Hub räumt "*.old" beim Start auf.
  Delete "$INSTDIR\paneldeck.exe.old"
  IfFileExists "$INSTDIR\paneldeck.exe" 0 +2
    Rename "$INSTDIR\paneldeck.exe" "$INSTDIR\paneldeck.exe.old"
  ClearErrors
  File /oname=paneldeck.exe "${EXE}"
  IfErrors 0 +3
    MessageBox MB_OK|MB_ICONSTOP "paneldeck.exe konnte nicht geschrieben werden. / Could not write paneldeck.exe." /SD IDOK
    Abort
  Delete /REBOOTOK "$INSTDIR\paneldeck.exe.old"
  File "..\README.md"
  File "..\README.de.md"
  File "..\Changelog.md"
  File "..\THIRD_PARTY.md"
  File "..\THIRD_PARTY_LICENSES.txt"
  File "..\LICENSE"
  SetOutPath "$INSTDIR\licenses"
  File "..\assets\fonts\LICENSE-Cascadia.txt"
  File "..\assets\fonts\LICENSE-NotoColorEmoji.txt"
  SetOutPath "$INSTDIR"
  WriteUninstaller "$INSTDIR\Uninstall.exe"

  CreateDirectory "$SMPROGRAMS\Paneldeck"
  CreateShortcut "$SMPROGRAMS\Paneldeck\Paneldeck.lnk" "$INSTDIR\paneldeck.exe"
  CreateShortcut "$SMPROGRAMS\Paneldeck\Paneldeck deinstallieren.lnk" "$INSTDIR\Uninstall.exe"

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

Section "Desktop-Verknüpfung" SecDesktop
  CreateShortcut "$DESKTOP\Paneldeck.lnk" "$INSTDIR\paneldeck.exe"
SectionEnd

Section /o "Mit Windows starten (minimiert im Tray)" SecAutostart
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "Paneldeck" '"$INSTDIR\paneldeck.exe" --tray'
SectionEnd

Section "Explorer-Kontextmenü „In Paneldeck öffnen“" SecContext
  WriteRegStr HKCU "Software\Classes\Directory\shell\Paneldeck" "" "In Paneldeck öffnen"
  WriteRegStr HKCU "Software\Classes\Directory\shell\Paneldeck" "Icon" '"$INSTDIR\paneldeck.exe"'
  WriteRegStr HKCU "Software\Classes\Directory\shell\Paneldeck\command" "" '"$INSTDIR\paneldeck.exe" --open "%1"'
  WriteRegStr HKCU "Software\Classes\Directory\Background\shell\Paneldeck" "" "In Paneldeck öffnen"
  WriteRegStr HKCU "Software\Classes\Directory\Background\shell\Paneldeck" "Icon" '"$INSTDIR\paneldeck.exe"'
  WriteRegStr HKCU "Software\Classes\Directory\Background\shell\Paneldeck\command" "" '"$INSTDIR\paneldeck.exe" --open "%V"'
  WriteRegStr HKCU "Software\Classes\Directory\shell\PaneldeckClaude" "" "Mit Claude in Paneldeck starten"
  WriteRegStr HKCU "Software\Classes\Directory\shell\PaneldeckClaude" "Icon" '"$INSTDIR\paneldeck.exe"'
  WriteRegStr HKCU "Software\Classes\Directory\shell\PaneldeckClaude\command" "" '"$INSTDIR\paneldeck.exe" --claude --open "%1"'
  WriteRegStr HKCU "Software\Classes\Directory\Background\shell\PaneldeckClaude" "" "Mit Claude in Paneldeck starten"
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

Section "Uninstall"
  ; Globale Claude-Hooks entfernen, solange die .exe noch da ist
  ExecWait '"$INSTDIR\paneldeck.exe" --uninstall-hooks'
  Delete "$INSTDIR\paneldeck.exe"
  Delete "$INSTDIR\paneldeck.exe.old"
  Delete "$INSTDIR\README.md"
  Delete "$INSTDIR\README.de.md"
  Delete "$INSTDIR\Changelog.md"
  Delete "$INSTDIR\THIRD_PARTY.md"
  Delete "$INSTDIR\THIRD_PARTY_LICENSES.txt"
  Delete "$INSTDIR\LICENSE"
  Delete "$INSTDIR\licenses\LICENSE-Cascadia.txt"
  Delete "$INSTDIR\licenses\LICENSE-NotoColorEmoji.txt"
  RMDir "$INSTDIR\licenses"
  Delete "$INSTDIR\Uninstall.exe"
  Delete "$SMPROGRAMS\Paneldeck\Paneldeck.lnk"
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
