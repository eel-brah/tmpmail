; tmpmail's additions to Tauri's NSIS installer (bundle > windows > nsis >
; installerHooks).
;
; Registers the tmpmail-desktop: link scheme for this user (or machine, with
; the install mode). A toast's click in the notification center can only
; start something: notify.rs makes it launch tmpmail-desktop://open?...,
; which starts the app with the link, and the single-instance plugin hands
; the link to the running app. Mirrors what the template does for deep links.

!macro NSIS_HOOK_POSTINSTALL
  WriteRegStr SHCTX "Software\Classes\tmpmail-desktop" "" "URL:tmpmail notification"
  WriteRegStr SHCTX "Software\Classes\tmpmail-desktop" "URL Protocol" ""
  WriteRegStr SHCTX "Software\Classes\tmpmail-desktop\DefaultIcon" "" "$\"$INSTDIR\${MAINBINARYNAME}.exe$\",0"
  WriteRegStr SHCTX "Software\Classes\tmpmail-desktop\shell\open\command" "" "$\"$INSTDIR\${MAINBINARYNAME}.exe$\" $\"%1$\""
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ; Only if it is still ours (another install may have taken it over).
  ReadRegStr $R7 SHCTX "Software\Classes\tmpmail-desktop\shell\open\command" ""
  ${If} $R7 == "$\"$INSTDIR\${MAINBINARYNAME}.exe$\" $\"%1$\""
    DeleteRegKey SHCTX "Software\Classes\tmpmail-desktop"
  ${EndIf}
!macroend
