; SideTone Installer
; Requires Inno Setup 6: https://jrsoftware.org/isdl.php
; Build: ISCC.exe installer.iss

#define AppName      "SideTone"
#define AppVersion   "7.0.0"
#define AppExe       "sidetone.exe"
#define AppPublisher "SideTone"
#define AppURL       ""

[Setup]
AppId={{E7C2A14F-3B8D-4F91-B2A6-9D5C1E4F7A83}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisherURL={#AppURL}
AppSupportURL={#AppURL}
DefaultDirName={localappdata}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
OutputDir=dist
OutputBaseFilename=SideTone-Setup-v{#AppVersion}
SetupIconFile=assets\sidetone.ico
Compression=lzma2/ultra64
SolidCompression=yes
WizardStyle=modern
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=commandline
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}
VersionInfoVersion={#AppVersion}
VersionInfoDescription={#AppName} Setup

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "Create a &desktop shortcut"; GroupDescription: "Additional icons:"

[Files]
Source: "target\release\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "assets\deps\yt-dlp.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "assets\deps\ffmpeg.exe"; DestDir: "{app}"; Flags: ignoreversion
; Legal: SideTone's MIT license + third-party notices (FFmpeg GPL source offer,
; yt-dlp Unlicense, Inter OFL). Required to ship alongside the bundled binaries.
Source: "LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion
Source: "THIRD-PARTY-NOTICES.md"; DestDir: "{app}"; DestName: "THIRD-PARTY-NOTICES.txt"; Flags: ignoreversion
; Exact GPLv3 text for the bundled FFmpeg binary (verbatim from gnu.org).
Source: "COPYING-GPL-3.0.txt"; DestDir: "{app}"; Flags: ignoreversion
; Bundle the Inter UI font so the app renders consistently.
Source: "assets\fonts\Inter.ttf"; DestDir: "{autofonts}"; FontInstall: "Inter"; Flags: onlyifdoesntexist uninsneveruninstall

[Icons]
Name: "{group}\{#AppName}"; Filename: "{app}\{#AppExe}"; IconFilename: "{app}\{#AppExe}"
Name: "{group}\Uninstall {#AppName}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; IconFilename: "{app}\{#AppExe}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#AppExe}"; Description: "Launch {#AppName}"; Flags: nowait postinstall skipifsilent
