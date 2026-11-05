; Hooks of the Windows installer (NSIS, see tauri.conf.json): "Ferry" in the
; "Send to" menu of File Explorer, removed again on uninstall.
!macro NSIS_HOOK_POSTINSTALL
  CreateShortCut "$SENDTO\Ferry.lnk" "$INSTDIR\ferry-gui.exe" "--" "$INSTDIR\ferry-gui.exe" 0
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  Delete "$SENDTO\Ferry.lnk"
!macroend
