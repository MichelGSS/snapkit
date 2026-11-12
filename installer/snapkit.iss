; SnapKit installer (Inno Setup 6).
; Built by scripts\package.ps1, which passes /DAppVersion=<version> and /DSourceDir=<staged folder>.
; Installs for the current user only, without asking for administrator rights.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef SourceDir
  #define SourceDir "..\dist\SnapKit"
#endif

[Setup]
; This identifier must not change between versions: it's how Windows recognizes updates.
AppId={{6B0E5C1F-2C1B-4B8B-9E4A-7C4D3F1A52E9}
AppName=SnapKit
AppVersion={#AppVersion}
AppPublisher=SnapKit contributors
DefaultDirName={localappdata}\Programs\SnapKit
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.19041
LicenseFile={#SourceDir}\LICENSE.txt
OutputDir=..\dist
OutputBaseFilename=SnapKit-{#AppVersion}-setup
SetupIconFile=..\assets\snapkit.ico
UninstallDisplayIcon={app}\snapkit.exe
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
; Closes SnapKit (which stays in the tray) before replacing the files.
CloseApplications=yes

[Languages]
Name: "en"; MessagesFile: "compiler:Default.isl"
Name: "ptbr"; MessagesFile: "compiler:Languages\BrazilianPortuguese.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#SourceDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{autoprograms}\SnapKit"; Filename: "{app}\snapkit.exe"
Name: "{userdesktop}\SnapKit"; Filename: "{app}\snapkit.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\snapkit.exe"; Description: "{cm:LaunchProgram,SnapKit}"; Flags: nowait postinstall skipifsilent

[Registry]
; If the user turned on "Start with Windows", the entry is removed on uninstall.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueName: "SnapKit"; Flags: dontcreatekey uninsdeletevalue
