# -*- coding: utf-8 -*-
Unicode true

!include "LogicLib.nsh"
!include "WinMessages.nsh"
!include "nsDialogs.nsh"

!ifndef PLUGIN_DIR
  !error "PLUGIN_DIR is required"
!endif
!ifndef THEME_DIR
  !error "THEME_DIR is required"
!endif
!ifndef OUTPUT_DIR
  !error "OUTPUT_DIR is required"
!endif

!addplugindir /x86-unicode "${PLUGIN_DIR}"

!ifdef DEMO_FAILURE
  Name "OneKey Modern Install Failure Demo"
  OutFile "${OUTPUT_DIR}\OneKeyModernFailureDemo.exe"
!else
  !ifdef DEMO_UNINSTALL
    Name "OneKey Modern Uninstall Demo"
    OutFile "${OUTPUT_DIR}\OneKeyModernUninstallDemo.exe"
  !else
    !ifdef DEMO_UNINSTALL_FAILURE
      Name "OneKey Modern Uninstall Failure Demo"
      OutFile "${OUTPUT_DIR}\OneKeyModernUninstallFailureDemo.exe"
    !else
      Name "OneKey Modern Installer Demo"
      OutFile "${OUTPUT_DIR}\OneKeyModernSetupDemo.exe"
    !endif
  !endif
!endif

InstallDir "$LOCALAPPDATA\Programs\OneKey"
RequestExecutionLevel user
AutoCloseWindow true
ShowInstDetails nevershow

Page instfiles

Var ModernUiActive
Var PluginResult
Var ProgressValue

!macro WaitForRecordingDelay Prefix
  !ifdef RECORDING_DELAY_MS
    StrCpy $ProgressValue 0
  ${Prefix}_recording_delay_loop:
    IntCmp $ProgressValue ${RECORDING_DELAY_MS} ${Prefix}_recording_delay_done 0 ${Prefix}_recording_delay_done
    nsis-duilib-ui::PollEvent
    Pop $PluginResult
    StrCmp $PluginResult "none" ${Prefix}_recording_delay_continue user_cancelled
  ${Prefix}_recording_delay_continue:
    Sleep 100
    IntOp $ProgressValue $ProgressValue + 100
    Goto ${Prefix}_recording_delay_loop
  ${Prefix}_recording_delay_done:
  !endif
!macroend

Section "Demo"
  InitPluginsDir
  SetOutPath "$PLUGINSDIR\onekey-modern"
  File /r "${THEME_DIR}\*.*"

!ifdef DEMO_FAILURE
  nsis-duilib-ui::Init "$PLUGINSDIR\onekey-modern" "zh-CN" "6.5.2"
  Pop $PluginResult
  StrCmp $PluginResult "ok" 0 fallback
  nsis-duilib-ui::SetPage "welcome"
  Pop $PluginResult
  StrCmp $PluginResult "ok" 0 fallback
  nsis-duilib-ui::SetInstallDirectory "$INSTDIR"
  Pop $PluginResult
  StrCmp $PluginResult "ok" 0 fallback
  nsis-duilib-ui::Show
  Pop $PluginResult
  StrCmp $PluginResult "ok" 0 fallback
  StrCpy $ModernUiActive "1"
  ShowWindow $HWNDPARENT ${SW_HIDE}

failure_welcome_loop:
  nsis-duilib-ui::WaitForEvent
  Pop $PluginResult
  StrCmp $PluginResult "primary" failure_begin_install
  StrCmp $PluginResult "primary-current" failure_begin_install
  StrCmp $PluginResult "primary-all" failure_begin_install
  StrCmp $PluginResult "browse" failure_choose_directory
  StrCmp $PluginResult "browse-current" failure_choose_directory
  StrCmp $PluginResult "browse-all" failure_choose_directory user_cancelled

failure_choose_directory:
  nsDialogs::SelectFolderDialog "Choose the OneKey installation folder" "$INSTDIR"
  Pop $0
  StrCmp $0 "" failure_welcome_loop
  StrCpy $INSTDIR "$0"
  nsis-duilib-ui::SetInstallDirectory "$INSTDIR"
  Pop $PluginResult
  StrCmp $PluginResult "ok" failure_welcome_loop fallback

failure_begin_install:
  nsis-duilib-ui::SetPage "installing"
  Pop $PluginResult
  StrCmp $PluginResult "ok" 0 fallback

  !insertmacro WaitForRecordingDelay failure

  StrCpy $ProgressValue 0

failure_progress_loop:
  IntCmp $ProgressValue 37 failure_show_error failure_progress_update failure_show_error
failure_progress_update:
  nsis-duilib-ui::SetStatus "Writing OneKey core components..."
  Pop $PluginResult
  nsis-duilib-ui::SetProgress "$ProgressValue"
  Pop $PluginResult
  nsis-duilib-ui::PollEvent
  Pop $PluginResult
  StrCmp $PluginResult "none" failure_continue_progress user_cancelled
failure_continue_progress:
  Sleep 120
  IntOp $ProgressValue $ProgressValue + 2
  Goto failure_progress_loop

failure_show_error:
  nsis-duilib-ui::SetPage "error"
  Pop $PluginResult
  StrCmp $PluginResult "ok" 0 fallback
  nsis-duilib-ui::WaitForEvent
  Pop $PluginResult
  StrCmp $PluginResult "primary" failure_retry user_cancelled

failure_retry:
  nsis-duilib-ui::ResetProgress "0"
  Pop $PluginResult
  nsis-duilib-ui::SetPage "welcome"
  Pop $PluginResult
  StrCmp $PluginResult "ok" 0 fallback
  nsis-duilib-ui::SetInstallDirectory "$INSTDIR"
  Pop $PluginResult
  StrCmp $PluginResult "ok" failure_welcome_loop fallback
!else
  !ifdef DEMO_UNINSTALL
  FileOpen $0 "$PLUGINSDIR\nsis-duilib-ui-uninstall-qa.tmp" w
  FileWrite $0 "Temporary QA payload"
  FileClose $0
  nsis-duilib-ui::Init "$PLUGINSDIR\onekey-modern" "zh-CN" "6.5.2"
  Pop $PluginResult
  StrCmp $PluginResult "ok" 0 fallback
  nsis-duilib-ui::SetPage "uninstalling"
  Pop $PluginResult
  StrCmp $PluginResult "ok" 0 fallback
  nsis-duilib-ui::Show
  Pop $PluginResult
  StrCmp $PluginResult "ok" 0 fallback
  StrCpy $ModernUiActive "1"
  ShowWindow $HWNDPARENT ${SW_HIDE}
  !insertmacro WaitForRecordingDelay uninstall
  StrCpy $ProgressValue 0

uninstall_progress_loop:
  IntCmp $ProgressValue 101 uninstall_done uninstall_progress_update uninstall_done
uninstall_progress_update:
  nsis-duilib-ui::SetStatus "Removing application components safely..."
  Pop $PluginResult
  nsis-duilib-ui::SetProgress "$ProgressValue"
  Pop $PluginResult
  nsis-duilib-ui::PollEvent
  Pop $PluginResult
  StrCmp $PluginResult "none" uninstall_continue_progress uninstall_cancelled
uninstall_continue_progress:
  Sleep 85
  IntOp $ProgressValue $ProgressValue + 2
  Goto uninstall_progress_loop

uninstall_done:
  Delete "$PLUGINSDIR\nsis-duilib-ui-uninstall-qa.tmp"
  nsis-duilib-ui::SetPage "uninstallFinish"
  Pop $PluginResult
  StrCmp $PluginResult "ok" 0 fallback
  nsis-duilib-ui::WaitForEvent
  Pop $PluginResult
  Goto demo_done

uninstall_cancelled:
  Delete "$PLUGINSDIR\nsis-duilib-ui-uninstall-qa.tmp"
  Goto user_cancelled
  !else
    !ifdef DEMO_UNINSTALL_FAILURE
  FileOpen $0 "$PLUGINSDIR\nsis-duilib-ui-uninstall-qa.tmp" w
  FileWrite $0 "Temporary QA payload"
  FileClose $0
  nsis-duilib-ui::Init "$PLUGINSDIR\onekey-modern" "zh-CN" "6.5.2"
  Pop $PluginResult
  StrCmp $PluginResult "ok" 0 fallback
  nsis-duilib-ui::SetPage "uninstalling"
  Pop $PluginResult
  StrCmp $PluginResult "ok" 0 fallback
  nsis-duilib-ui::Show
  Pop $PluginResult
  StrCmp $PluginResult "ok" 0 fallback
  StrCpy $ModernUiActive "1"
  ShowWindow $HWNDPARENT ${SW_HIDE}
  !insertmacro WaitForRecordingDelay uninstall_failure
  StrCpy $ProgressValue 0

uninstall_failure_progress_loop:
  IntCmp $ProgressValue 43 uninstall_show_error uninstall_failure_progress_update uninstall_show_error
uninstall_failure_progress_update:
  nsis-duilib-ui::SetStatus "Removing application components safely..."
  Pop $PluginResult
  nsis-duilib-ui::SetProgress "$ProgressValue"
  Pop $PluginResult
  nsis-duilib-ui::PollEvent
  Pop $PluginResult
  StrCmp $PluginResult "none" uninstall_failure_continue uninstall_failure_cancelled
uninstall_failure_continue:
  Sleep 100
  IntOp $ProgressValue $ProgressValue + 2
  Goto uninstall_failure_progress_loop

uninstall_show_error:
  Delete "$PLUGINSDIR\nsis-duilib-ui-uninstall-qa.tmp"
  nsis-duilib-ui::SetPage "uninstallError"
  Pop $PluginResult
  StrCmp $PluginResult "ok" 0 fallback
  nsis-duilib-ui::WaitForEvent
  Pop $PluginResult
  StrCmp $PluginResult "primary" uninstall_failure_retry user_cancelled

uninstall_failure_retry:
  FileOpen $0 "$PLUGINSDIR\nsis-duilib-ui-uninstall-qa.tmp" w
  FileWrite $0 "Temporary QA payload"
  FileClose $0
  nsis-duilib-ui::ResetProgress "0"
  Pop $PluginResult
  nsis-duilib-ui::SetStatus "Removing application components safely..."
  Pop $PluginResult
  nsis-duilib-ui::SetPage "uninstalling"
  Pop $PluginResult
  StrCmp $PluginResult "ok" 0 fallback
  !insertmacro WaitForRecordingDelay uninstall_failure_retry
  StrCpy $ProgressValue 0
  Goto uninstall_failure_progress_loop

uninstall_failure_cancelled:
  Delete "$PLUGINSDIR\nsis-duilib-ui-uninstall-qa.tmp"
  Goto user_cancelled
    !else
  nsis-duilib-ui::Init "$PLUGINSDIR\onekey-modern" "zh-CN" "6.5.2"
  Pop $PluginResult
  StrCmp $PluginResult "ok" 0 fallback
  nsis-duilib-ui::SetPage "welcome"
  Pop $PluginResult
  StrCmp $PluginResult "ok" 0 fallback
  nsis-duilib-ui::SetInstallDirectory "$INSTDIR"
  Pop $PluginResult
  StrCmp $PluginResult "ok" 0 fallback
  nsis-duilib-ui::Show
  Pop $PluginResult
  StrCmp $PluginResult "ok" 0 fallback
  StrCpy $ModernUiActive "1"
  ShowWindow $HWNDPARENT ${SW_HIDE}

welcome_loop:
  nsis-duilib-ui::WaitForEvent
  Pop $PluginResult
  StrCmp $PluginResult "primary" begin_install
  StrCmp $PluginResult "primary-current" begin_install
  StrCmp $PluginResult "primary-all" begin_install
  StrCmp $PluginResult "browse" choose_install_directory
  StrCmp $PluginResult "browse-current" choose_install_directory
  StrCmp $PluginResult "browse-all" choose_install_directory user_cancelled

choose_install_directory:
  nsDialogs::SelectFolderDialog "Choose the OneKey installation folder" "$INSTDIR"
  Pop $0
  StrCmp $0 "" welcome_loop
  StrCpy $INSTDIR "$0"
  nsis-duilib-ui::SetInstallDirectory "$INSTDIR"
  Pop $PluginResult
  StrCmp $PluginResult "ok" welcome_loop fallback

begin_install:
  nsis-duilib-ui::SetPage "installing"
  Pop $PluginResult
  StrCmp $PluginResult "ok" 0 fallback

  !insertmacro WaitForRecordingDelay setup

  StrCpy $ProgressValue 0
progress_loop:
  IntCmp $ProgressValue 101 progress_done progress_update progress_done
progress_update:
  StrCmp $ProgressValue 0 status_prepare
  StrCmp $ProgressValue 36 status_extract
  StrCmp $ProgressValue 72 status_finalize
  Goto status_done
status_prepare:
  nsis-duilib-ui::SetStatus "Preparing installation files..."
  Pop $PluginResult
  Goto status_done
status_extract:
  nsis-duilib-ui::SetStatus "Writing OneKey core components..."
  Pop $PluginResult
  Goto status_done
status_finalize:
  nsis-duilib-ui::SetStatus "Finishing the last setup steps..."
  Pop $PluginResult
status_done:
  nsis-duilib-ui::SetProgress "$ProgressValue"
  Pop $PluginResult
  nsis-duilib-ui::PollEvent
  Pop $PluginResult
  StrCmp $PluginResult "none" continue_progress
  Goto user_cancelled
continue_progress:
  Sleep 100
  IntOp $ProgressValue $ProgressValue + 2
  Goto progress_loop

progress_done:
  nsis-duilib-ui::SetPage "finish"
  Pop $PluginResult
  StrCmp $PluginResult "ok" 0 fallback
  nsis-duilib-ui::WaitForEvent
  Pop $PluginResult
  Goto demo_done
    !endif
  !endif
!endif

!ifndef DEMO_FAILURE
  !ifndef DEMO_UNINSTALL_FAILURE
demo_done:
    nsis-duilib-ui::Shutdown
    Pop $PluginResult
    Quit
  !endif
!endif

user_cancelled:
  nsis-duilib-ui::Shutdown
  Pop $PluginResult
  Quit

fallback:
  nsis-duilib-ui::Shutdown
  Pop $0
  ShowWindow $HWNDPARENT ${SW_SHOW}
  DetailPrint "Modern UI unavailable; using native NSIS: $PluginResult"
  Sleep 800
SectionEnd
