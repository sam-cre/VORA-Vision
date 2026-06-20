!include "MUI2.nsh"
!include "nsDialogs.nsh"
!include "LogicLib.nsh"

Name "VORA-Vision"
OutFile "VORA-Vision-Installer.exe"
InstallDir "$PROGRAMFILES\VORA-Vision"
RequestExecutionLevel admin

; Global variables for input values
Var FredKey
Var FinnhubKey

; Modern UI settings
!define MUI_ABORTWARNING
!define MUI_ICON "${NSISDIR}\Contrib\Graphics\Icons\classic-install.ico"
!define MUI_UNICON "${NSISDIR}\Contrib\Graphics\Icons\classic-uninstall.ico"

; Pages
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_LICENSE "EULA.txt"
!insertmacro MUI_PAGE_DIRECTORY

; Custom page for API Keys
Page custom APIKeysPageCreate APIKeysPageLeave

!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_WELCOME
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_UNPAGE_FINISH

!insertmacro MUI_LANGUAGE "English"

; custom page controls
Var Dialog
Var FredInput
Var FinnhubInput

Function APIKeysPageCreate
    nsDialogs::Create 1018
    Pop $Dialog

    ${If} $Dialog == error
        Abort
    ${EndIf}

    ${NSD_CreateLabel} 0 0 100% 24u "VORA-Vision API Configuration$\r$\nConfigure free API keys to enable live data fetching."
    Pop $0

    ; FRED API Key input
    ${NSD_CreateLabel} 0 30u 100% 10u "FRED API Key (Get at: https://fred.stlouisfed.org):"
    Pop $0
    ${NSD_CreateText} 0 42u 100% 12u ""
    Pop $FredInput

    ; Finnhub API Key input
    ${NSD_CreateLabel} 0 62u 100% 10u "Finnhub API Key (Get at: https://finnhub.io):"
    Pop $0
    ${NSD_CreateText} 0 74u 100% 12u ""
    Pop $FinnhubInput

    ${NSD_CreateLabel} 0 94u 100% 20u "Keys will be saved to '.env' in your installation folder. You can modify them at any time."
    Pop $0

    nsDialogs::Show
FunctionEnd

Function APIKeysPageLeave
    ${NSD_GetText} $FredInput $FredKey
    ${NSD_GetText} $FinnhubInput $FinnhubKey
FunctionEnd

Section "Install"
    SetOutPath "$INSTDIR"
    
    ; Pack only the compiled executable (EULA and .env are generated or packaged cleanly)
    File "target\release\vora-vision.exe"
    File "EULA.txt"

    ; Write EULA/TOS configured .env file
    FileOpen $0 "$INSTDIR\.env" w
    FileWrite $0 "FRED_API_KEY=$FredKey$\r$\n"
    FileWrite $0 "FINNHUB_API_KEY=$FinnhubKey$\r$\n"
    FileClose $0

    ; Shortcuts
    CreateDirectory "$SMPROGRAMS\VORA-Vision"
    CreateShortcut "$SMPROGRAMS\VORA-Vision\VORA-Vision.lnk" "$INSTDIR\vora-vision.exe"
    CreateShortcut "$SMPROGRAMS\VORA-Vision\Uninstall.lnk" "$INSTDIR\uninstall.exe"
    CreateShortcut "$DESKTOP\VORA-Vision.lnk" "$INSTDIR\vora-vision.exe"

    WriteUninstaller "$INSTDIR\uninstall.exe"
SectionEnd

Section "Uninstall"
    Delete "$INSTDIR\.env"
    Delete "$INSTDIR\vora-vision.exe"
    Delete "$INSTDIR\EULA.txt"
    Delete "$INSTDIR\uninstall.exe"
    RMDir "$INSTDIR"

    Delete "$SMPROGRAMS\VORA-Vision\VORA-Vision.lnk"
    Delete "$SMPROGRAMS\VORA-Vision\Uninstall.lnk"
    RMDir "$SMPROGRAMS\VORA-Vision"
    Delete "$DESKTOP\VORA-Vision.lnk"
SectionEnd
