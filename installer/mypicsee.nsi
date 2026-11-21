; Build with NSIS 3.x:
; makensis -DINPUT_DIR=<staging> -DOUTPUT_FILE=<installer.exe> -DAPP_VERSION=20261122.001 -DAPP_PE_VERSION=2026.11.22.1 mypicsee.nsi
; Staging inputs: 3wa_mypicsee.exe, LICENSE, THIRD_PARTY_NOTICES.txt, app.ico.
Unicode true
RequestExecutionLevel user
SetCompressor /SOLID lzma

!ifndef INPUT_DIR
  !error "Pass -DINPUT_DIR=<staging directory>"
!endif
!ifndef OUTPUT_FILE
  !error "Pass -DOUTPUT_FILE=<installer.exe>"
!endif
!ifndef APP_VERSION
  !define APP_VERSION "20261122.001"
!endif
!ifndef APP_PE_VERSION
  !define APP_PE_VERSION "2026.11.22.1"
!endif

!define APP_ID "3wa_mypicsee"
!define APP_EXE "3wa_mypicsee.exe"
!define APP_PROGID "3wa.MyPicSee.Image"
!define APP_INSTALLDIR "$LOCALAPPDATA\3wa\3wa_mypicsee"
!define APP_UNINSTALLKEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\3wa_mypicsee"

!include "MUI2.nsh"
!include "LogicLib.nsh"
!include "nsDialogs.nsh"
!include "x64.nsh"

Name "看快寶 ${APP_VERSION}"
OutFile "${OUTPUT_FILE}"
InstallDir "${APP_INSTALLDIR}"
BrandingText "3wa_mypicsee"
VIProductVersion "${APP_PE_VERSION}"
VIAddVersionKey /LANG=1028 "ProductName" "3wa_mypicsee"
VIAddVersionKey /LANG=1028 "FileDescription" "看快寶 (3wa_mypicsee) 安裝程式"
VIAddVersionKey /LANG=1028 "FileVersion" "${APP_VERSION}"
VIAddVersionKey /LANG=1028 "ProductVersion" "${APP_VERSION}"
VIAddVersionKey /LANG=1028 "CompanyName" "3wa"
VIAddVersionKey /LANG=1028 "LegalCopyright" "Copyright © 2026 shadowjohn"

!define MUI_ICON "${INPUT_DIR}/app.ico"
!define MUI_UNICON "${INPUT_DIR}/app.ico"
!define MUI_ABORTWARNING
!define MUI_WELCOMEPAGE_TITLE "安裝看快寶"
!define MUI_WELCOMEPAGE_TEXT "看快寶只會安裝到目前 Windows 帳號，不需要管理員權限。下一步可選擇要加到「開啟方式」的圖片格式，不會替換預設看圖程式。"
!insertmacro MUI_PAGE_WELCOME
Page custom AssocPageCreate AssocPageLeave
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "TradChinese"

Var JpegBox
Var PngBox
Var WebpBox
Var GifBox
Var TiffBox
Var BmpBox
Var JpegSelected
Var PngSelected
Var WebpSelected
Var GifSelected
Var TiffSelected
Var BmpSelected

!macro Association State Extension
  ${If} $${State} == ${BST_CHECKED}
    WriteRegStr HKCU "Software\Classes\${Extension}\OpenWithProgids" "${APP_PROGID}" ""
  ${Else}
    !insertmacro RemoveAssociation "${Extension}"
  ${EndIf}
!macroend

!macro RemoveAssociation Extension
  DeleteRegValue HKCU "Software\Classes\${Extension}\OpenWithProgids" "${APP_PROGID}"
!macroend

Function .onInit
  ${IfNot} ${RunningX64}
    MessageBox MB_OK|MB_ICONSTOP "此版本需使用 64 位元 Windows。" /SD IDOK
    Quit
  ${EndIf}
  SetShellVarContext current
  SetRegView 64
  ; Fixed per-user location, including when a command line /D argument is supplied.
  StrCpy $INSTDIR "${APP_INSTALLDIR}"
  StrCpy $JpegSelected ${BST_UNCHECKED}
  StrCpy $PngSelected ${BST_UNCHECKED}
  StrCpy $WebpSelected ${BST_UNCHECKED}
  StrCpy $GifSelected ${BST_UNCHECKED}
  StrCpy $TiffSelected ${BST_UNCHECKED}
  StrCpy $BmpSelected ${BST_UNCHECKED}
FunctionEnd

Function AssocPageCreate
  !insertmacro MUI_HEADER_TEXT "選擇圖片格式" "將看快寶加入選定格式的「開啟方式」"
  nsDialogs::Create 1018
  Pop $0
  ${If} $0 == error
    Abort
  ${EndIf}
  ${NSD_CreateLabel} 0 0 100% 21u "全部預設不勾選。選這些項目也不會改變 Windows 的預設應用程式。"
  Pop $0
  ${NSD_CreateCheckbox} 10u 30u 94% 12u "JPEG (.jpg, .jpeg)"
  Pop $JpegBox
  ${NSD_CreateCheckbox} 10u 47u 94% 12u "PNG (.png)"
  Pop $PngBox
  ${NSD_CreateCheckbox} 10u 64u 94% 12u "WebP (.webp)"
  Pop $WebpBox
  ${NSD_CreateCheckbox} 10u 81u 94% 12u "GIF (.gif)"
  Pop $GifBox
  ${NSD_CreateCheckbox} 10u 98u 94% 12u "TIFF (.tif, .tiff)"
  Pop $TiffBox
  ${NSD_CreateCheckbox} 10u 115u 94% 12u "BMP (.bmp)"
  Pop $BmpBox
  ${NSD_SetState} $JpegBox $JpegSelected
  ${NSD_SetState} $PngBox $PngSelected
  ${NSD_SetState} $WebpBox $WebpSelected
  ${NSD_SetState} $GifBox $GifSelected
  ${NSD_SetState} $TiffBox $TiffSelected
  ${NSD_SetState} $BmpBox $BmpSelected
  nsDialogs::Show
FunctionEnd

Function AssocPageLeave
  ${NSD_GetState} $JpegBox $JpegSelected
  ${NSD_GetState} $PngBox $PngSelected
  ${NSD_GetState} $WebpBox $WebpSelected
  ${NSD_GetState} $GifBox $GifSelected
  ${NSD_GetState} $TiffBox $TiffSelected
  ${NSD_GetState} $BmpBox $BmpSelected
FunctionEnd

Section "看快寶" MainInstall
  SetOutPath "$INSTDIR"
  SetOverwrite on
  ClearErrors
  File "${INPUT_DIR}/${APP_EXE}"
  File "${INPUT_DIR}/LICENSE"
  File "${INPUT_DIR}/THIRD_PARTY_NOTICES.txt"
  File "${__FILEDIR__}/NSIS_NOTICES.txt"
  WriteUninstaller "$INSTDIR\Uninstall.exe"
  ${If} ${Errors}
    MessageBox MB_OK|MB_ICONSTOP "安裝檔案失敗。若看快寶已開啟，請關閉後再試。" /SD IDOK
    SetErrorLevel 2
    Abort
  ${EndIf}

  CreateShortcut "$SMPROGRAMS\看快寶.lnk" "$INSTDIR\${APP_EXE}"

  WriteRegStr HKCU "${APP_UNINSTALLKEY}" "DisplayName" "看快寶 (3wa_mypicsee)"
  WriteRegStr HKCU "${APP_UNINSTALLKEY}" "DisplayVersion" "${APP_VERSION}"
  WriteRegStr HKCU "${APP_UNINSTALLKEY}" "Publisher" "3wa"
  WriteRegStr HKCU "${APP_UNINSTALLKEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${APP_UNINSTALLKEY}" "DisplayIcon" "$INSTDIR\${APP_EXE},0"
  WriteRegStr HKCU "${APP_UNINSTALLKEY}" "UninstallString" '$\"$INSTDIR\Uninstall.exe$\"'
  WriteRegStr HKCU "${APP_UNINSTALLKEY}" "QuietUninstallString" '$\"$INSTDIR\Uninstall.exe$\" /S'
  WriteRegDWORD HKCU "${APP_UNINSTALLKEY}" "NoModify" 1
  WriteRegDWORD HKCU "${APP_UNINSTALLKEY}" "NoRepair" 1

  IfSilent associations_done
  ; Register a private ProgID only when a format group is explicitly selected.
  ; Never set an extension's default value or Windows' UserChoice.
  ${If} $JpegSelected == ${BST_CHECKED}
  ${OrIf} $PngSelected == ${BST_CHECKED}
  ${OrIf} $WebpSelected == ${BST_CHECKED}
  ${OrIf} $GifSelected == ${BST_CHECKED}
  ${OrIf} $TiffSelected == ${BST_CHECKED}
  ${OrIf} $BmpSelected == ${BST_CHECKED}
    WriteRegStr HKCU "Software\Classes\${APP_PROGID}" "" "看快寶圖片"
    WriteRegStr HKCU "Software\Classes\${APP_PROGID}\DefaultIcon" "" '$\"$INSTDIR\${APP_EXE}$\",0'
    WriteRegStr HKCU "Software\Classes\${APP_PROGID}\shell\open\command" "" '$\"$INSTDIR\${APP_EXE}$\" $\"%1$\"'
  ${Else}
    DeleteRegKey HKCU "Software\Classes\${APP_PROGID}"
  ${EndIf}
  !insertmacro Association JpegSelected ".jpg"
  !insertmacro Association JpegSelected ".jpeg"
  !insertmacro Association PngSelected ".png"
  !insertmacro Association WebpSelected ".webp"
  !insertmacro Association GifSelected ".gif"
  !insertmacro Association TiffSelected ".tif"
  !insertmacro Association TiffSelected ".tiff"
  !insertmacro Association BmpSelected ".bmp"
  associations_done:
  System::Call 'shell32::SHChangeNotify(i 0x08000000, i 0, p 0, p 0)'
SectionEnd

Function un.onInit
  SetShellVarContext current
  SetRegView 64
  ; Do not allow an arbitrary path supplied via the command line.
  StrCmp $INSTDIR "${APP_INSTALLDIR}" path_ok
    MessageBox MB_OK|MB_ICONSTOP "安裝目錄不符，未解除安裝。" /SD IDOK
    Quit
  path_ok:
FunctionEnd

Section "Uninstall"
  ClearErrors
  Delete "$INSTDIR\${APP_EXE}"
  ${If} ${Errors}
    MessageBox MB_OK|MB_ICONSTOP "請先關閉看快寶，再解除安裝。" /SD IDOK
    SetErrorLevel 2
    Abort
  ${EndIf}

  !insertmacro RemoveAssociation ".jpg"
  !insertmacro RemoveAssociation ".jpeg"
  !insertmacro RemoveAssociation ".png"
  !insertmacro RemoveAssociation ".webp"
  !insertmacro RemoveAssociation ".gif"
  !insertmacro RemoveAssociation ".tif"
  !insertmacro RemoveAssociation ".tiff"
  !insertmacro RemoveAssociation ".bmp"
  DeleteRegKey HKCU "Software\Classes\${APP_PROGID}"
  DeleteRegKey HKCU "${APP_UNINSTALLKEY}"
  System::Call 'shell32::SHChangeNotify(i 0x08000000, i 0, p 0, p 0)'

  Delete "$SMPROGRAMS\看快寶.lnk"
  Delete "$INSTDIR\LICENSE"
  Delete "$INSTDIR\THIRD_PARTY_NOTICES.txt"
  Delete "$INSTDIR\NSIS_NOTICES.txt"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir "$INSTDIR"
SectionEnd
