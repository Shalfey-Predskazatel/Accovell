Unicode true

!include "MUI2.nsh"
!include "LogicLib.nsh"

!define PRODUCT_NAME "Codex Accounts"
!define PRODUCT_VERSION "0.1.0"
!define PRODUCT_PUBLISHER "Codex Accounts"
!define PRODUCT_KEY "Software\CodexAccounts"
!define UNINSTALL_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\CodexAccounts"
!define ROOT "${__FILEDIR__}\.."
!define ASSETS "${__FILEDIR__}\assets"
!define APP_ICON "${ROOT}\src-tauri\icons\icon.ico"
!define MUI_ICON "${ROOT}\src-tauri\icons\icon.ico"
!define APP_WEBVIEW2_GUID "{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}"

!ifndef APP_BIN
!define APP_BIN "${ROOT}\src-tauri\target\release\codex-accounts.exe"
!endif
!ifndef APP_WEBVIEW2_LOADER
!define APP_WEBVIEW2_LOADER "${ROOT}\src-tauri\target\release\WebView2Loader.dll"
!endif
!ifndef APP_WEBVIEW2_BOOTSTRAPPER
!define APP_WEBVIEW2_BOOTSTRAPPER "${ASSETS}\MicrosoftEdgeWebView2Setup.exe"
!endif
!ifndef INSTALLER_OUTFILE
!define INSTALLER_OUTFILE "${ROOT}\outputs\CodexAccounts-icon-test.exe"
!endif

Name "${PRODUCT_NAME} ${PRODUCT_VERSION}"
Caption "${PRODUCT_NAME} Setup"
OutFile "${INSTALLER_OUTFILE}"
InstallDir "$LOCALAPPDATA\Programs\Codex Accounts"
InstallDirRegKey HKCU "${PRODUCT_KEY}" "InstallDir"
RequestExecutionLevel user
SilentInstall silent
ShowInstDetails nevershow
ShowUninstDetails nevershow
SetCompressor /SOLID lzma
SetCompressorDictSize 16
SetOverwrite on
WindowIcon on
Icon "${APP_ICON}"

VIProductVersion "0.1.0.0"
VIAddVersionKey "ProductName" "${PRODUCT_NAME}"
VIAddVersionKey "ProductVersion" "${PRODUCT_VERSION}"
VIAddVersionKey "FileDescription" "Codex Accounts profile manager"
VIAddVersionKey "CompanyName" "${PRODUCT_PUBLISHER}"
VIAddVersionKey "FileVersion" "${PRODUCT_VERSION}"
VIAddVersionKey "LegalCopyright" "Copyright 2026 Codex Accounts"

LangString AppMustClose 1049 "Закройте Codex Accounts перед установкой или обновлением."
LangString AppMustClose 1033 "Close Codex Accounts before installing or updating."
LangString InstallFailed 1049 "Установка не завершена. Запустите установщик ещё раз."
LangString InstallFailed 1033 "Installation was not completed. Run the installer again."
LangString UninstallFailed 1049 "Удаление не завершено. Профили и настройки не изменены."
LangString UninstallFailed 1033 "Removal was not completed. Profiles and settings were not changed."
LangString UninstallTitle 1049 "Удаление Codex Accounts"
LangString UninstallTitle 1033 "Uninstall Codex Accounts"
LangString UninstallText 1049 "Будут удалены файлы приложения и ярлыки. Профили и настройки пользователя сохранятся."
LangString UninstallText 1033 "Application files and shortcuts will be removed. Your profiles and settings will be kept."
LangString UninstallFinishTitle 1049 "Codex Accounts удалён"
LangString UninstallFinishTitle 1033 "Codex Accounts was removed"
LangString UninstallFinishText 1049 "Профили и настройки пользователя сохранены."
LangString UninstallFinishText 1033 "Your profiles and settings were kept."

!define MUI_UNWELCOMEPAGE_TITLE "$(UninstallTitle)"
!define MUI_UNWELCOMEPAGE_TEXT "$(UninstallText)"
!define MUI_UNFINISHPAGE_TITLE "$(UninstallFinishTitle)"
!define MUI_UNFINISHPAGE_TEXT "$(UninstallFinishText)"
!insertmacro MUI_UNPAGE_WELCOME
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_UNPAGE_FINISH
!insertmacro MUI_LANGUAGE "Russian"
!insertmacro MUI_LANGUAGE "English"

Var INSTALL_STATUS
Var UNINSTALL_STATUS

Function .onInit
    SetShellVarContext current
    ; The visible setup is the Tauri screen. NSIS is only the transport shell.
    SetSilent silent
    InitPluginsDir

    ; Keep the bootstrap executable beside WebView2Loader.dll so Tauri can
    ; start even before the payload is copied to the selected folder.
    SetOutPath "$PLUGINSDIR"
    File /oname=codex-accounts-installer.exe "${APP_BIN}"
    File /oname=WebView2Loader.dll "${APP_WEBVIEW2_LOADER}"
    File /oname=MicrosoftEdgeWebView2Setup.exe "${APP_WEBVIEW2_BOOTSTRAPPER}"

    ; The Rust installer copies this directory and creates shortcuts.
    SetOutPath "$PLUGINSDIR\payload"
    File /oname=codex-accounts.exe "${APP_BIN}"
    File /oname=WebView2Loader.dll "${APP_WEBVIEW2_LOADER}"
    File /oname=MicrosoftEdgeWebView2Setup.exe "${APP_WEBVIEW2_BOOTSTRAPPER}"
    File /oname=icon.ico "${APP_ICON}"

    SetOutPath "$PLUGINSDIR\payload\licenses"
    File /oname=ibm-plex-sans-LICENSE.txt "${ROOT}\installer\third_party\IBMPlexSans-LICENSE.txt"
    File /oname=THIRD_PARTY_NOTICES.md "${ROOT}\installer\third_party\nsis-duilib-ui\THIRD_PARTY_NOTICES.md"
    File /oname=nsis-duilib-ui-LICENSE.txt "${ROOT}\installer\third_party\nsis-duilib-ui\LICENSE"
    File /oname=duilib-LICENSE.txt "${ROOT}\installer\third_party\nsis-duilib-ui\third_party\duilib\LICENSE"
    File /oname=json-LICENSE.txt "${ROOT}\installer\third_party\nsis-duilib-ui\third_party\nlohmann\LICENSE.MIT"
    File /oname=nsis-headers-LICENSE.txt "${ROOT}\installer\third_party\nsis-duilib-ui\third_party\nsis\LICENSE"

    ExecWait '"$PLUGINSDIR\codex-accounts-installer.exe" --installer --payload-dir "$PLUGINSDIR\payload" --result-file "$PLUGINSDIR\installer-result.ini" --webview2-bootstrapper "$PLUGINSDIR\MicrosoftEdgeWebView2Setup.exe" --install-dir "$INSTDIR"' $0

    ReadINIStr $INSTALL_STATUS "$PLUGINSDIR\installer-result.ini" installer status
    ${If} $INSTALL_STATUS != "installed"
        Quit
    ${EndIf}
    ReadINIStr $INSTDIR "$PLUGINSDIR\installer-result.ini" installer path
    ${If} $INSTDIR == ""
        MessageBox MB_OK|MB_ICONSTOP "$(InstallFailed)"
        Quit
    ${EndIf}
FunctionEnd

Section "Register installation"
    WriteUninstaller "$INSTDIR\Uninstall.exe"
    CreateDirectory "$SMPROGRAMS\Codex Accounts"
    CreateShortcut "$SMPROGRAMS\Codex Accounts\Uninstall Codex Accounts.lnk" "$INSTDIR\Uninstall.exe" "" "$INSTDIR\icon.ico" 0
    WriteRegStr HKCU "${PRODUCT_KEY}" "InstallDir" "$INSTDIR"
    WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayName" "${PRODUCT_NAME}"
    WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayVersion" "${PRODUCT_VERSION}"
    WriteRegStr HKCU "${UNINSTALL_KEY}" "Publisher" "${PRODUCT_PUBLISHER}"
    WriteRegStr HKCU "${UNINSTALL_KEY}" "InstallLocation" "$INSTDIR"
    WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayIcon" "$INSTDIR\icon.ico"
    WriteRegStr HKCU "${UNINSTALL_KEY}" "UninstallString" "$\"$INSTDIR\Uninstall.exe$\""
    WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoModify" 1
    WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoRepair" 1
SectionEnd

Function un.onInit
    SetShellVarContext current
    SetSilent silent
    InitPluginsDir
    System::Call 'kernel32::GetUserDefaultUILanguage() i.r0'
    ${If} $0 == 1049
        StrCpy $LANGUAGE 1049
    ${Else}
        StrCpy $LANGUAGE 1033
    ${EndIf}
    CopyFiles /SILENT "$INSTDIR\codex-accounts.exe" "$PLUGINSDIR"
    CopyFiles /SILENT "$INSTDIR\WebView2Loader.dll" "$PLUGINSDIR"
    ExecWait '"$PLUGINSDIR\codex-accounts.exe" --installer --uninstall --install-dir "$INSTDIR" --result-file "$PLUGINSDIR\uninstall-result.ini"' $0
    ReadINIStr $UNINSTALL_STATUS "$PLUGINSDIR\uninstall-result.ini" installer status
    ${If} $UNINSTALL_STATUS == "cancelled"
        Quit
    ${EndIf}
    ${If} $UNINSTALL_STATUS != "uninstalled"
        MessageBox MB_OK|MB_ICONEXCLAMATION "$(UninstallFailed)"
        Quit
    ${EndIf}
FunctionEnd

Function un.CheckApplicationClosed
    ; The application uses Tauri's single-instance guard. If it is active,
    ; prepare-for-uninstall below returns a clear error and aborts safely.
FunctionEnd

Section "Uninstall"
    Call un.CheckApplicationClosed
    Delete "$SMPROGRAMS\Codex Accounts\Codex Accounts.lnk"
    Delete "$SMPROGRAMS\Codex Accounts\Uninstall Codex Accounts.lnk"
    RMDir "$SMPROGRAMS\Codex Accounts"
    Delete "$DESKTOP\Codex Accounts.lnk"
    Delete "$INSTDIR\codex-accounts.exe"
    Delete "$INSTDIR\WebView2Loader.dll"
    Delete "$INSTDIR\MicrosoftEdgeWebView2Setup.exe"
    Delete "$INSTDIR\icon.ico"
    Delete "$INSTDIR\Uninstall.exe"
    Delete "$INSTDIR\licenses\nsis-duilib-ui-LICENSE.txt"
    Delete "$INSTDIR\licenses\THIRD_PARTY_NOTICES.md"
    Delete "$INSTDIR\licenses\duilib-LICENSE.txt"
    Delete "$INSTDIR\licenses\json-LICENSE.txt"
    Delete "$INSTDIR\licenses\nsis-headers-LICENSE.txt"
    Delete "$INSTDIR\licenses\ibm-plex-sans-LICENSE.txt"
    RMDir "$INSTDIR\licenses"
    RMDir "$INSTDIR"
    DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "CodexAccounts"
    DeleteRegKey HKCU "${UNINSTALL_KEY}"
    DeleteRegKey HKCU "${PRODUCT_KEY}"
SectionEnd





