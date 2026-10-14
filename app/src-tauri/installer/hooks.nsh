; Subwoofa's additions to the Tauri installer. The page is slotted in by installer.nsi (a copy of Tauri's template with
; one line added); the rest runs from Tauri's hook points.
;
;   the Extras page      two optional steps, both chosen with a checkbox: put Subwoofa on the PATH, download FFmpeg
;   after the install    does what was ticked (fetch-ffmpeg.ps1 is installed beside the program)
;   before the uninstall undoes both, and only what this installer added

Var OptPath
Var OptFfmpeg
Var OptPathBox
Var OptFfmpegBox
Var OptFfmpegFound

; Is there an FFmpeg already: on PATH, or a copy this installer fetched before?
!macro SUBWOOFA_FIND_FFMPEG
  StrCpy $OptFfmpegFound 0
  nsExec::ExecToStack 'cmd /c where ffmpeg'
  Pop $0
  Pop $1
  ${If} $0 == 0
    StrCpy $OptFfmpegFound 1
  ${EndIf}
  ${If} ${FileExists} "$LOCALAPPDATA\Subwoofa\ffmpeg\ffmpeg.exe"
    StrCpy $OptFfmpegFound 1
  ${EndIf}
!macroend

!macro SUBWOOFA_OPTIONS_PAGE
  Page custom SubwoofaOptions SubwoofaOptionsLeave

  Function SubwoofaOptions
    ; passive and silent installs show no pages and never change the PATH or download anything unasked
    ${If} $PassiveMode = 1
    ${OrIf} ${Silent}
      Abort
    ${EndIf}
    !insertmacro MUI_HEADER_TEXT "Extras" "Two optional steps. Skip both and Subwoofa still works."
    nsDialogs::Create 1018
    Pop $0
    ${If} $0 == error
      Abort
    ${EndIf}
    !insertmacro SUBWOOFA_FIND_FFMPEG

    ${NSD_CreateCheckbox} 0 4u 100% 12u "Add Subwoofa to PATH"
    Pop $OptPathBox
    ${NSD_Check} $OptPathBox
    ${NSD_CreateLabel} 14u 18u 95% 24u "Lets you type  subwoofa  in any command prompt or PowerShell window, from any folder. Only your account is changed, and uninstalling takes it out again."
    Pop $0

    ${NSD_CreateCheckbox} 0 52u 100% 12u "Download FFmpeg now (about 115 MB)"
    Pop $OptFfmpegBox
    ${If} $OptFfmpegFound == 1
      EnableWindow $OptFfmpegBox 0
      ${NSD_CreateLabel} 14u 66u 95% 24u "FFmpeg was found on this PC, so there is nothing to download."
    ${Else}
      ${NSD_Check} $OptFfmpegBox
      ${NSD_CreateLabel} 14u 66u 95% 36u "Subwoofa uses FFmpeg to read most input files and to make a few formats. This fetches the Windows build the FFmpeg website links to (gyan.dev), checks it against its published checksum, and keeps it in Subwoofa's own folder. You can also do this later in Settings, under Engines."
    ${EndIf}
    Pop $0
    nsDialogs::Show
  FunctionEnd

  Function SubwoofaOptionsLeave
    ${NSD_GetState} $OptPathBox $OptPath
    ${NSD_GetState} $OptFfmpegBox $OptFfmpeg
  FunctionEnd
!macroend

!macro NSIS_HOOK_POSTINSTALL
  ${If} $OptPath == ${BST_CHECKED}
    DetailPrint "Adding Subwoofa to your PATH..."
    nsExec::ExecToLog 'powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$INSTDIR\path.ps1" -Action add -Dir "$INSTDIR"'
    Pop $0
    ${If} $0 != 0
      DetailPrint "The PATH could not be changed; Subwoofa still works from its own folder."
    ${EndIf}
  ${EndIf}

  ${If} $OptFfmpeg == ${BST_CHECKED}
    DetailPrint "Downloading FFmpeg (about 115 MB)..."
    nsExec::ExecToLog 'powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$INSTDIR\fetch-ffmpeg.ps1"'
    Pop $0
    ${If} $0 != 0
      MessageBox MB_OK|MB_ICONEXCLAMATION "FFmpeg could not be downloaded. The reason is in the details list and in the file $TEMP\subwoofa-ffmpeg-log.txt (open it in Notepad to copy it). Subwoofa is installed and works without it; you can try again in Settings, under Engines." /SD IDOK
    ${EndIf}
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ; take this install's folder out of the PATH, leaving every other entry as it was (an update puts it back itself)
  ${If} $UpdateMode <> 1
    nsExec::ExecToLog 'powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$INSTDIR\path.ps1" -Action remove -Dir "$INSTDIR"'
    Pop $0
  ${EndIf}

  ; the FFmpeg that Subwoofa downloaded goes with the application data, when "Delete the application data" is ticked
  ${If} $DeleteAppDataCheckboxState = 1
  ${AndIf} $UpdateMode <> 1
    RMDir /r "$LOCALAPPDATA\Subwoofa\ffmpeg"
    RMDir "$LOCALAPPDATA\Subwoofa"
  ${EndIf}
!macroend
