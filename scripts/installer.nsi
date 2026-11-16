Unicode true
RequestExecutionLevel user
!include "MUI2.nsh"
!include "x64.nsh"

!define UninstallKey "Software\Microsoft\Windows\CurrentVersion\Uninstall\PadPointer"
Name "PadPointer ${VERSION}"
OutFile "${SETUP_EXE}"
InstallDir "$LOCALAPPDATA\Programs\PadPointer"
InstallDirRegKey HKCU "${UninstallKey}" "InstallLocation"
SetCompressor /SOLID lzma
VIProductVersion "${VERSION}.0"
VIAddVersionKey "ProductName" "PadPointer"
VIAddVersionKey "FileDescription" "PadPointer Windows x64 Setup"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "ProductVersion" ""
VIAddVersionKey "LegalCopyright" "Copyright (c) 2026 PadPointer contributors"

!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "SimpChinese"
!insertmacro MUI_LANGUAGE "English"

Function .onInit
    ${IfNot} ${RunningX64}
        MessageBox MB_ICONSTOP "PadPointer requires Windows x64."
        Abort
    ${EndIf}
    SetRegView 64
FunctionEnd

Section
    SetShellVarContext current
    SetOutPath "$INSTDIR"
    File "${RELEASE_DIR}\PadPointer.exe"
    File "${RELEASE_DIR}\LICENSE"
    File "${NOTICE_ZIP}"
    WriteUninstaller "$INSTDIR\Uninstall.exe"
    CreateShortcut "$SMPROGRAMS\PadPointer.lnk" "$INSTDIR\PadPointer.exe"
    WriteRegStr HKCU "${UninstallKey}" "DisplayName" "PadPointer"
    WriteRegStr HKCU "${UninstallKey}" "DisplayVersion" "${VERSION}"
    WriteRegStr HKCU "${UninstallKey}" "InstallLocation" "$INSTDIR"
    WriteRegStr HKCU "${UninstallKey}" "DisplayIcon" "$INSTDIR\PadPointer.exe"
    WriteRegStr HKCU "${UninstallKey}" "UninstallString" '"$INSTDIR\Uninstall.exe"'
    WriteRegDWORD HKCU "${UninstallKey}" "NoModify" 1
    WriteRegDWORD HKCU "${UninstallKey}" "NoRepair" 1
SectionEnd

Section "Uninstall"
    SetRegView 64
    SetShellVarContext current
    Delete "$SMPROGRAMS\PadPointer.lnk"
    Delete "$INSTDIR\PadPointer.exe"
    Delete "$INSTDIR\LICENSE"
    Delete "$INSTDIR\ThirdPartyLicenses.zip"
    Delete "$INSTDIR\Uninstall.exe"
    RMDir "$INSTDIR"
    DeleteRegKey HKCU "${UninstallKey}"
SectionEnd


