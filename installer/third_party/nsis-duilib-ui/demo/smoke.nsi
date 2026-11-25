# -*- coding: utf-8 -*-
Unicode true

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

Name "nsis-duilib-ui smoke test"
OutFile "${OUTPUT_DIR}\NsisDuiSmoke.exe"
RequestExecutionLevel user
SilentInstall silent

Section
  Delete "$TEMP\nsis-duilib-ui-smoke.txt"
  InitPluginsDir
  SetOutPath "$PLUGINSDIR\onekey-modern"
  File /r "${THEME_DIR}\*.*"

  nsis-duilib-ui::Init "$PLUGINSDIR\onekey-modern" "zh-CN" "6.5.2"
  Pop $0
  nsis-duilib-ui::Shutdown
  Pop $1
  nsis-duilib-ui::GetVersion
  Pop $1

  FileOpen $2 "$TEMP\nsis-duilib-ui-smoke.txt" w
  FileWrite $2 "init=$0$\r$\nversion=$1$\r$\n"
  FileClose $2
  StrCmp $0 "ok" success
  SetErrorLevel 2
success:
SectionEnd
