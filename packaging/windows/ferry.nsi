# Bản cài Windows của Ferry (ferry-gd1_14, bê `groove/packaging/windows/groove.nsi`). Dựng bằng
# `packaging/windows/build-installer.ps1`, không gọi tay: script truyền VERSION, VERSION4, STAGE, ICON, OUTFILE qua /D và đọc tệp
# này với /INPUTCHARSET UTF8.
#
# Cài cho cả máy, CẦN admin (khác Groovex): luật tường lửa cho mDNS (UDP 5353 vào, chỉ cho `ferry.exe`) phải ghi lúc cài — thiếu
# thì dò điện thoại không chạy trên mạng "Public" (Windows mặc định coi WiFi mới là Public). Không ký (ký là dịch vụ trả phí;
# SmartScreen sẽ cảnh báo).
#
# Hai exe cạnh nhau: `ferry.exe` (khay, daemon bật `ferry-ui.exe` nằm cạnh nó) và `ferry-ui.exe` (cửa sổ). Tự chạy cùng máy
# (`crates/daemon/src/autostart.rs`) là tác vụ Task Scheduler `ferry-<user>` do app ghi khi người dùng bật — trỏ đường exe cài
# ở đây; gỡ cài xoá tác vụ của người đang gỡ. Ferry đang chạy: exe bị giữ ⇒ hỏi thoát từ khay rồi thử lại; cài im lặng thì dừng
# mã 2. Gỡ cài hỏi có xoá cấu hình, hàng đợi không, mặc định giữ; token ghép điện thoại ở Credential Manager không đụng.

Unicode true
SetCompressor /SOLID lzma
ManifestDPIAware true
RequestExecutionLevel admin

!include "MUI2.nsh"
!include "Sections.nsh"
!include "FileFunc.nsh"
!include "x64.nsh"

!define APP_NAME "Ferry"
; D-21: nhà phát hành, trang chủ chung mọi app.
!define PUBLISHER "z0 team, sandtechs jsc."
!define HOMEPAGE "https://sandtechs.com"
!define UNINST_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\Ferry"
!define UNINST_EXE "uninstall.exe"
!define FIREWALL_RULE "Ferry (mDNS)"
!define START_LINK "$SMPROGRAMS\Ferry.lnk"
!define DESKTOP_LINK "$DESKTOP\Ferry.lnk"
# 250 ms mỗi lượt: chờ tối đa 5 s.
!define LOCK_ROUNDS 20

Name "${APP_NAME}"
OutFile "${OUTFILE}"
InstallDir "$PROGRAMFILES64\Ferry"
InstallDirRegKey HKLM "${UNINST_KEY}" "InstallLocation"
BrandingText "Ferry ${VERSION}"

VIProductVersion "${VERSION4}"
VIAddVersionKey /LANG=0 "ProductName" "Ferry"
VIAddVersionKey /LANG=0 "CompanyName" "${PUBLISHER}"
VIAddVersionKey /LANG=0 "ProductVersion" "${VERSION}"
VIAddVersionKey /LANG=0 "FileVersion" "${VERSION}"
VIAddVersionKey /LANG=0 "FileDescription" "Ferry setup"
VIAddVersionKey /LANG=0 "LegalCopyright" "Copyright © 2026 Sandtechs"

!define MUI_ICON "${ICON}"
!define MUI_UNICON "${ICON}"
!define MUI_ABORTWARNING
!define MUI_COMPONENTSPAGE_NODESC
# Chạy app sau khi cài: trình cài đang là admin ⇒ `Exec` thẳng sẽ chạy Ferry với quyền admin. Mở qua Explorer để app chạy với
# quyền người dùng thường.
!define MUI_FINISHPAGE_RUN
!define MUI_FINISHPAGE_RUN_TEXT "$(run_ferry)"
!define MUI_FINISHPAGE_RUN_FUNCTION RunAsUser

!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

# Ngôn ngữ đầu là mặc định khi máy không dùng ngôn ngữ nào trong danh sách.
!insertmacro MUI_LANGUAGE "English"
!insertmacro MUI_LANGUAGE "Vietnamese"

LangString run_ferry ${LANG_ENGLISH} "Open Ferry"
LangString run_ferry ${LANG_VIETNAMESE} "Mở Ferry"
LangString still_running ${LANG_ENGLISH} "Ferry is still running. Quit it from the tray icon (Quit), then choose Retry."
LangString still_running ${LANG_VIETNAMESE} "Ferry vẫn đang chạy. Hãy thoát từ icon ở khay (Thoát) rồi chọn Retry."
LangString remove_data ${LANG_ENGLISH} "Also delete Ferry settings and the send queue?$\n$\nFiles on this computer and on your phone are never deleted. Choose No to keep them for a later install."
LangString remove_data ${LANG_VIETNAMESE} "Xoá luôn cài đặt và hàng đợi gửi của Ferry?$\n$\nTệp trên máy và trên điện thoại không bao giờ bị xoá. Chọn No để giữ lại cho lần cài sau."
LangString app_description ${LANG_ENGLISH} "Send files and folders to your phone over WiFi"
LangString app_description ${LANG_VIETNAMESE} "Gửi tệp và thư mục sang điện thoại qua WiFi"
LangString sec_app ${LANG_ENGLISH} "Ferry (required)"
LangString sec_app ${LANG_VIETNAMESE} "Ferry (bắt buộc)"
LangString sec_desktop ${LANG_ENGLISH} "Desktop shortcut"
LangString sec_desktop ${LANG_VIETNAMESE} "Lối tắt trên Desktop"
LangString need_x64 ${LANG_ENGLISH} "Ferry needs 64-bit Windows."
LangString need_x64 ${LANG_VIETNAMESE} "Ferry cần Windows 64-bit."

# Chờ một exe trong thư mục cài được nhả (FileOpen chế độ "a" báo lỗi khi exe đang chạy; tệp đã soát là có nên không tạo mới).
!macro WaitUnlocked EXE
  IfFileExists "$INSTDIR\${EXE}" 0 +3
    StrCpy $R1 "$INSTDIR\${EXE}"
    Call ${__WAIT_PREFIX}WaitFile
!macroend

!macro WaitFileFunction PREFIX
Function ${PREFIX}WaitFile
  StrCpy $R0 0
  poll:
    ClearErrors
    FileOpen $R2 $R1 a
    IfErrors locked
    FileClose $R2
    Goto done
  locked:
    IntOp $R0 $R0 + 1
    IntCmp $R0 ${LOCK_ROUNDS} ask
    Sleep 250
    Goto poll
  ask:
    MessageBox MB_RETRYCANCEL|MB_ICONEXCLAMATION "$(still_running)" /SD IDCANCEL IDRETRY retry
    SetErrorLevel 2
    Quit
  retry:
    StrCpy $R0 0
    Goto poll
  done:
FunctionEnd
!macroend

!insertmacro WaitFileFunction ""
!insertmacro WaitFileFunction "un."

!macro StopApp PREFIX
  !define __WAIT_PREFIX "${PREFIX}"
  !insertmacro WaitUnlocked "ferry.exe"
  !insertmacro WaitUnlocked "ferry-ui.exe"
  !undef __WAIT_PREFIX
!macroend

# Luật cũ cùng tên (cài đè, đổi thư mục cài) xoá trước rồi ghi lại, trỏ đúng exe mới. Lỗi chỉ ghi chi tiết: app vẫn chạy được trên
# mạng Private.
!macro AddFirewallRule
  nsExec::Exec 'netsh advfirewall firewall delete rule name="${FIREWALL_RULE}"'
  Pop $0
  nsExec::ExecToLog 'netsh advfirewall firewall add rule name="${FIREWALL_RULE}" dir=in action=allow program="$INSTDIR\ferry.exe" protocol=UDP localport=5353 profile=any description="Ferry: find phones on the local network (mDNS)"'
  Pop $0
  StrCmp $0 0 +2
    DetailPrint "firewall rule: netsh exit $0"
!macroend

Function RunAsUser
  Exec '"$WINDIR\explorer.exe" "$INSTDIR\ferry.exe"'
FunctionEnd

Section "$(sec_app)" SecMain
  SectionIn RO
  SetShellVarContext all
  !insertmacro StopApp ""

  SetOutPath "$INSTDIR"
  File "${STAGE}\ferry.exe"
  File "${STAGE}\ferry-ui.exe"
  File "${STAGE}\ferry.ico"

  # `uninstall.exe` chứng tỏ thư mục này do bộ cài đặt: `/D=` trỏ nhầm chỗ có sẵn `licenses/` của người dùng thì không xoá.
  IfFileExists "$INSTDIR\${UNINST_EXE}" 0 +2
    RMDir /r "$INSTDIR\licenses"
  SetOutPath "$INSTDIR\licenses"
  File /r "${STAGE}\licenses\*"

  SetOutPath "$INSTDIR"
  WriteUninstaller "$INSTDIR\${UNINST_EXE}"
  CreateShortcut "${START_LINK}" "$INSTDIR\ferry.exe" "" "$INSTDIR\ferry.ico" 0 SW_SHOWNORMAL "" "$(app_description)"
  !insertmacro AddFirewallRule

  WriteRegStr HKLM "${UNINST_KEY}" "DisplayName" "Ferry"
  WriteRegStr HKLM "${UNINST_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKLM "${UNINST_KEY}" "Publisher" "${PUBLISHER}"
  WriteRegStr HKLM "${UNINST_KEY}" "URLInfoAbout" "${HOMEPAGE}"
  WriteRegStr HKLM "${UNINST_KEY}" "DisplayIcon" "$INSTDIR\ferry.ico"
  WriteRegStr HKLM "${UNINST_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKLM "${UNINST_KEY}" "UninstallString" '"$INSTDIR\${UNINST_EXE}"'
  WriteRegStr HKLM "${UNINST_KEY}" "QuietUninstallString" '"$INSTDIR\${UNINST_EXE}" /S'
  WriteRegStr HKLM "${UNINST_KEY}" "Comments" "$(app_description)"
  WriteRegDWORD HKLM "${UNINST_KEY}" "NoModify" 1
  WriteRegDWORD HKLM "${UNINST_KEY}" "NoRepair" 1
  SectionGetSize ${SecMain} $0
  WriteRegDWORD HKLM "${UNINST_KEY}" "EstimatedSize" $0
SectionEnd

Section /o "$(sec_desktop)" SecDesktop
  SetShellVarContext all
  CreateShortcut "${DESKTOP_LINK}" "$INSTDIR\ferry.exe" "" "$INSTDIR\ferry.ico" 0 SW_SHOWNORMAL "" "$(app_description)"
SectionEnd

Function .onInit
  ${IfNot} ${RunningX64}
    MessageBox MB_ICONSTOP "$(need_x64)" /SD IDOK
    Abort
  ${EndIf}
  SetRegView 64
  # Cài đè: giữ lựa chọn lối tắt Desktop của lần trước.
  SetShellVarContext all
  IfFileExists "${DESKTOP_LINK}" 0 +2
    SectionSetFlags ${SecDesktop} ${SF_SELECTED}
FunctionEnd

# Tự cập nhật (ferry-gd2_01): daemon chạy `_setup.exe /S /UPDATE /D=<thư mục cài>` rồi thoát ⇒ StopApp chờ exe hết khoá, cài
# xong bật lại Ferry qua Explorer (không chạy app bằng quyền admin của bộ cài) — app mở cửa sổ, hộp "Có gì mới" hiện. Cài
# im lặng thường (không `/UPDATE`) không bật app.
Function .onInstSuccess
  ${GetParameters} $R0
  ClearErrors
  ${GetOptions} $R0 "/UPDATE" $R1
  IfErrors +2
    Call RunAsUser
FunctionEnd

Function un.onInit
  SetRegView 64
FunctionEnd

Section "Uninstall"
  SetShellVarContext all
  !insertmacro StopApp "un."

  nsExec::Exec 'netsh advfirewall firewall delete rule name="${FIREWALL_RULE}"'
  Pop $0
  # Tác vụ tự chạy của người đang gỡ (`autostart::task_name`: `ferry-<USERNAME chữ thường, ký tự lạ thành _>`). Người dùng khác
  # trên máy: tác vụ còn lại trỏ exe đã mất, Task Scheduler báo lỗi im lặng.
  nsExec::Exec `powershell -NoProfile -NonInteractive -Command "$$n = ($$env:USERNAME.ToLower() -replace '[^a-z0-9]', '_'); schtasks /Delete /TN ('ferry-' + $$n) /F"`
  Pop $0

  Delete "${START_LINK}"
  Delete "${DESKTOP_LINK}"
  DeleteRegKey HKLM "${UNINST_KEY}"

  Delete "$INSTDIR\ferry.exe"
  Delete "$INSTDIR\ferry-ui.exe"
  Delete "$INSTDIR\ferry.ico"
  RMDir /r "$INSTDIR\licenses"
  Delete "$INSTDIR\${UNINST_EXE}"
  # Không /r: thư mục còn tệp lạ thì để nguyên.
  RMDir "$INSTDIR"

  # Dữ liệu của người đang gỡ (`ferry_core::paths`): cấu hình ở Roaming, hàng đợi + log ở Local. Gỡ im lặng thì giữ.
  SetShellVarContext current
  MessageBox MB_YESNO|MB_ICONQUESTION|MB_DEFBUTTON2 "$(remove_data)" /SD IDNO IDNO keep_data
    RMDir /r "$APPDATA\ferry"
    RMDir /r "$LOCALAPPDATA\ferry"
  keep_data:
SectionEnd
