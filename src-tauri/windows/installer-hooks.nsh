; This hook runs before the Tauri template copies or removes the sidecar.
; The script checks ExecutablePath, so a development Agent or another install
; with the same process name is never selected for termination.
!define WT_MEDIA_HOOK_DIR "${__FILEDIR__}"
!macro WT_MEDIA_STOP_INSTALLED_AGENT
  ; The template checks the main process after PREINSTALL/PREUNINSTALL. Stop it
  ; first here so it cannot start a new Agent while the sidecar is being removed.
  !insertmacro CheckIfAppIsRunning "${MAINBINARYNAME}.exe" "${PRODUCTNAME}"

  InitPluginsDir
  SetOutPath "$PLUGINSDIR"
  File "${WT_MEDIA_HOOK_DIR}\stop-installed-agent.ps1"
  SetOutPath "$INSTDIR"
  nsExec::ExecToLog '"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$PLUGINSDIR\stop-installed-agent.ps1" "$INSTDIR\wt-media-agent.exe"'
  Pop $R0
  ${If} $R0 != 0
    SetErrorLevel 1
    Abort "无法停止当前安装目录下的 Agent，安装或卸载已中止（退出码 $R0）。"
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREINSTALL
  !insertmacro WT_MEDIA_STOP_INSTALLED_AGENT
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  !insertmacro WT_MEDIA_STOP_INSTALLED_AGENT
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ; PRODUCTNAME is defined after the hook include, before macro expansion.
; Managed data roots follow the package identity, never Cloud routing.
!if "${PRODUCTNAME}" == "起飞 Dev"
  !define WT_MEDIA_DATA_ROOT "WTMedia\Dev"
!else if "${PRODUCTNAME}" == "起飞 Test"
  !define WT_MEDIA_DATA_ROOT "WTMedia\Test"
!else if "${PRODUCTNAME}" == "起飞"
  !define WT_MEDIA_DATA_ROOT "WTMedia"
!else
  !error "Unknown Desktop identity: refusing unscoped data cleanup"
!endif


  ${If} ${FileExists} "$INSTDIR\wt-media-agent.exe"
    SetErrorLevel 1
    Abort "Agent 程序仍在安装目录，卸载未完成；请检查文件占用与权限。"
  ${EndIf}

  ; Tauri's checkbox only removes the bundle-id directory. Our runtime roots
  ; have separate Desktop/Agent paths. Never touch the operator's save folders.
  ${If} $DeleteAppDataCheckboxState = 1
  ${AndIf} $UpdateMode != 1
    SetShellVarContext current
    RMDir /r "$LOCALAPPDATA\${WT_MEDIA_DATA_ROOT}\Desktop"
    RMDir /r "$LOCALAPPDATA\${WT_MEDIA_DATA_ROOT}\Agent"
    RMDir "$LOCALAPPDATA\WTMedia"

    ; Before RC15, installed Windows builds could use the macOS-shaped
    ; per-user paths. Remove only the two WTMedia component subtrees.
    RMDir /r "$PROFILE\Library\Application Support\${WT_MEDIA_DATA_ROOT}\Desktop"
    RMDir /r "$PROFILE\Library\Application Support\${WT_MEDIA_DATA_ROOT}\Agent"
    RMDir "$PROFILE\Library\Application Support\WTMedia"
    RMDir /r "$PROFILE\Library\Logs\${WT_MEDIA_DATA_ROOT}\Desktop"
    RMDir /r "$PROFILE\Library\Logs\${WT_MEDIA_DATA_ROOT}\Agent"
    RMDir "$PROFILE\Library\Logs\WTMedia"
    RMDir /r "$PROFILE\Library\Caches\${WT_MEDIA_DATA_ROOT}\Desktop"
    RMDir "$PROFILE\Library\Caches\WTMedia"

    ${If} ${FileExists} "$LOCALAPPDATA\${WT_MEDIA_DATA_ROOT}\Desktop"
    ${OrIf} ${FileExists} "$LOCALAPPDATA\${WT_MEDIA_DATA_ROOT}\Agent"
    ${OrIf} ${FileExists} "$PROFILE\Library\Application Support\${WT_MEDIA_DATA_ROOT}\Desktop"
    ${OrIf} ${FileExists} "$PROFILE\Library\Application Support\${WT_MEDIA_DATA_ROOT}\Agent"
    ${OrIf} ${FileExists} "$PROFILE\Library\Logs\${WT_MEDIA_DATA_ROOT}\Desktop"
    ${OrIf} ${FileExists} "$PROFILE\Library\Logs\${WT_MEDIA_DATA_ROOT}\Agent"
    ${OrIf} ${FileExists} "$PROFILE\Library\Caches\${WT_MEDIA_DATA_ROOT}\Desktop"
      SetErrorLevel 1
      Abort "应用数据或日志未能完全清理，请检查文件占用与权限。"
    ${EndIf}
  ${EndIf}
!macroend
