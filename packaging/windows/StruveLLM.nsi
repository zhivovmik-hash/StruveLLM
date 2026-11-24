Unicode true
RequestExecutionLevel user
ManifestDPIAware true
!include "MUI2.nsh"
!include "LogicLib.nsh"
!include "FileFunc.nsh"
!include "x64.nsh"
!define PRODUCT "Struve LLM Alpha"
!define REGKEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\StruveLLMAlpha"
!define WEBKEY "Software\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}"
!define MARK ".struve-llm-alpha.ini"
Name "${PRODUCT} ${VERSION}"
OutFile "${OUTFILE}"
VIProductVersion "${FILEVERSION}"
VIAddVersionKey "ProductName" "Struve LLM"
VIAddVersionKey "FileDescription" "Struve LLM Alpha Setup"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "ProductVersion" "${VERSION}"
VIAddVersionKey "LegalCopyright" "Struve LLM contributors"
InstallDir "$LOCALAPPDATA\Programs\Struve LLM Alpha"
InstallDirRegKey HKCU "${REGKEY}" "InstallLocation"
SetCompressor zlib
CRCCheck force
Icon "${PAYLOAD}\app\struve-green.ico"
UninstallIcon "${PAYLOAD}\app\struve-green.ico"
!define MUI_ABORTWARNING
!define MUI_LANGDLL_ALWAYSSHOW
!define MUI_LANGDLL_ALLLANGUAGES
!define MUI_LANGDLL_WINDOWTITLE "Struve LLM — Language / Язык"
!define MUI_LANGDLL_INFO "Select a language / Выберите язык"
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"
!insertmacro MUI_LANGUAGE "Russian"
LangString Busy ${LANG_RUSSIAN} "Закройте Struve LLM через меню значка в трее → Выход. Завершите CLI этой установки и повторите."
LangString Busy ${LANG_ENGLISH} "Exit Struve LLM from the tray and close the CLI for this installation, then retry."
LangString Folder ${LANG_RUSSIAN} "Выберите пустую папку для альфы или её прежнюю папку. Для переноса сначала удалите установленную копию."
LangString Folder ${LANG_ENGLISH} "Choose an empty folder or the original alpha folder. To relocate, uninstall the previous copy first."
LangString WebFailed ${LANG_RUSSIAN} "Окну нужен Microsoft Edge WebView2 Runtime. Установите Evergreen x64 по инструкции первого запуска и повторите."
LangString WebFailed ${LANG_ENGLISH} "The desktop requires Microsoft Edge WebView2 Runtime. Install Evergreen x64 as described in the quick start and retry."
LangString Need64 ${LANG_RUSSIAN} "Для этой сборки требуется 64-разрядная Windows x64."
LangString Need64 ${LANG_ENGLISH} "This build requires x64 Windows."
LangString PathLong ${LANG_RUSSIAN} "Путь к установке слишком длинный. Выберите более короткую папку."
LangString PathLong ${LANG_ENGLISH} "Please choose a shorter installation path."

Function .onInit
  !insertmacro MUI_LANGDLL_DISPLAY
  ${If} ${Silent}
    ${GetParameters} $0
    ${GetOptions} $0 "/LANG=" $1
    ${If} $1 == "en"
      StrCpy $LANGUAGE ${LANG_ENGLISH}
    ${ElseIf} $1 == "ru"
      StrCpy $LANGUAGE ${LANG_RUSSIAN}
    ${EndIf}
  ${EndIf}
  ${IfNot} ${RunningX64}
    MessageBox MB_ICONSTOP "$(Need64)" /SD IDOK
    Abort
  ${EndIf}
FunctionEnd

!macro CheckClosed EXE
  ${If} ${FileExists} "$INSTDIR\app\${EXE}"
    ClearErrors
    FileOpen $0 "$INSTDIR\app\${EXE}" a
    ${If} ${Errors}
      MessageBox MB_ICONSTOP "$(Busy)" /SD IDOK
      Abort
    ${EndIf}
    FileClose $0
  ${EndIf}
!macroend

Function CheckFolder
  StrLen $0 "$INSTDIR"
  ${If} $0 > ${MAXROOT}
    MessageBox MB_ICONSTOP "$(PathLong)" /SD IDOK
    Abort
  ${EndIf}
  ReadRegStr $0 HKCU "${REGKEY}" "InstallLocation"
  ${If} $0 != ""
  ${AndIf} $0 != $INSTDIR
    MessageBox MB_ICONSTOP "$(Folder)" /SD IDOK
    Abort
  ${EndIf}
  ReadINIStr $0 "$INSTDIR\${MARK}" "Product" "Id"
  ${If} $0 != "org.struve.llm.alpha"
    FindFirst $0 $1 "$INSTDIR\*"
    ${DoWhile} $1 != ""
      ${If} $1 != "."
      ${AndIf} $1 != ".."
        FindClose $0
        MessageBox MB_ICONSTOP "$(Folder)" /SD IDOK
        Abort
      ${EndIf}
      FindNext $0 $1
    ${Loop}
    FindClose $0
  ${EndIf}
  !insertmacro CheckClosed "struve-desktop.exe"
  !insertmacro CheckClosed "struve-llm.exe"
FunctionEnd

Function HasWebView
  SetRegView 32
  ReadRegStr $0 HKLM "${WEBKEY}" "pv"
  ${If} $0 == ""
  ${OrIf} $0 == "0.0.0.0"
    ReadRegStr $0 HKCU "${WEBKEY}" "pv"
  ${EndIf}
  SetRegView lastused
FunctionEnd

Section "Struve LLM"
  Call CheckFolder
  Call HasWebView
  ${If} $0 == ""
  ${OrIf} $0 == "0.0.0.0"
    InitPluginsDir
    SetOutPath "$PLUGINSDIR"
    File /oname=webview2setup.exe "${WEBBOOT}"
    ExecWait '"$PLUGINSDIR\webview2setup.exe" /silent /install' $1
    Call HasWebView
    ${If} $0 == ""
    ${OrIf} $0 == "0.0.0.0"
      MessageBox MB_ICONSTOP "$(WebFailed)" /SD IDOK
      Abort
    ${EndIf}
  ${EndIf}
  SetShellVarContext current
  SetOutPath "$INSTDIR"
  WriteINIStr "$INSTDIR\${MARK}" "Product" "Id" "org.struve.llm.alpha"
  WriteINIStr "$INSTDIR\${MARK}" "Product" "Language" "$LANGUAGE"
  WriteUninstaller "$INSTDIR\Uninstall.exe"
  File /r "${PAYLOAD}\*.*"
  ClearErrors
  FileOpen $0 "$INSTDIR\app\installer-language.txt" w
  ${If} ${Errors}
    Abort
  ${EndIf}
  ${If} $LANGUAGE == ${LANG_RUSSIAN}
    FileWrite $0 "ru"
  ${Else}
    FileWrite $0 "en"
  ${EndIf}
  FileClose $0
  ${If} ${Errors}
    Abort
  ${EndIf}
  CreateShortcut "$SMPROGRAMS\${PRODUCT}.lnk" "$INSTDIR\app\struve-desktop.exe" "" "$INSTDIR\app\struve-green.ico"
  CreateShortcut "$DESKTOP\${PRODUCT}.lnk" "$INSTDIR\app\struve-desktop.exe" "" "$INSTDIR\app\struve-green.ico"
  WriteRegStr HKCU "${REGKEY}" "DisplayName" "${PRODUCT}"
  WriteRegStr HKCU "${REGKEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${REGKEY}" "DisplayIcon" "$INSTDIR\app\struve-green.ico"
  WriteRegStr HKCU "${REGKEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${REGKEY}" "UninstallString" '"$INSTDIR\Uninstall.exe"'
  WriteRegDWORD HKCU "${REGKEY}" "NoModify" 1
  WriteRegDWORD HKCU "${REGKEY}" "NoRepair" 1
SectionEnd

Function un.onInit
  ReadINIStr $0 "$INSTDIR\${MARK}" "Product" "Id"
  ${If} $0 != "org.struve.llm.alpha"
    Abort
  ${EndIf}
  ReadINIStr $LANGUAGE "$INSTDIR\${MARK}" "Product" "Language"
FunctionEnd
Section "Uninstall"
  !insertmacro CheckClosed "struve-desktop.exe"
  !insertmacro CheckClosed "struve-llm.exe"
  SetShellVarContext current
  Delete "$INSTDIR\app\installer-language.txt"
  !include "${UNINSTALL_LIST}"
  Delete "$SMPROGRAMS\${PRODUCT}.lnk"
  Delete "$DESKTOP\${PRODUCT}.lnk"
  ReadRegStr $0 HKCU "${REGKEY}" "InstallLocation"
  ${If} $0 == $INSTDIR
    DeleteRegKey HKCU "${REGKEY}"
  ${EndIf}
  Delete "$INSTDIR\Uninstall.exe"
  Delete "$INSTDIR\${MARK}"
  ClearErrors
  RMDir "$INSTDIR"
  ${If} ${Errors}
    WriteINIStr "$INSTDIR\${MARK}" "Product" "Id" "org.struve.llm.alpha"
  ${EndIf}
SectionEnd
