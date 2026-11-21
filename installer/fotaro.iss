; Fotaro's Windows installer (Inno Setup 6). Build it with installer\build.ps1, which
; passes the version; or: ISCC.exe /DAppVersion=0.5.0 installer\fotaro.iss
;
; Installs for the current user (no administrator rights), so Fotaro can update itself:
; it downloads a newer installer and runs it with /SILENT once it has closed, and the
; installer starts the new Fotaro when it is done.

#ifndef AppVersion
  #error Pass the version: ISCC.exe /DAppVersion=x.y.z fotaro.iss
#endif
#define Root SourcePath + "\.."
#ifndef Exe
  #define Exe Root + "\target\release\fotaro.exe"
#endif
#ifndef OutputDir
  #define OutputDir Root + "\target\installer"
#endif

[Setup]
; Never change the AppId: it is how an update finds the installed Fotaro.
AppId={{CBA8D7F0-7F59-4EF9-9F86-94EF9A8E245B}
AppName=Fotaro
AppVersion={#AppVersion}
AppVerName=Fotaro {#AppVersion}
AppPublisher=The Fotaro authors
AppPublisherURL=https://github.com/jboneng/Fotaro
AppSupportURL=https://github.com/jboneng/Fotaro/issues
AppUpdatesURL=https://github.com/jboneng/Fotaro/releases
DefaultDirName={autopf}\Fotaro
DisableProgramGroupPage=yes
DisableDirPage=auto
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
OutputDir={#OutputDir}
OutputBaseFilename=Fotaro-{#AppVersion}-windows-x86_64-setup
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
UninstallDisplayIcon={app}\fotaro.exe
UninstallDisplayName=Fotaro
; Fotaro closes before an update runs; if one is still open, it is asked to close.
CloseApplications=yes
RestartApplications=no

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#Exe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Root}\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion
Source: "{#Root}\README.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Fotaro"; Filename: "{app}\fotaro.exe"
Name: "{autodesktop}\Fotaro"; Filename: "{app}\fotaro.exe"; Tasks: desktopicon

[Run]
; Installed by hand: offer to start Fotaro. Updated by Fotaro (silently): start it again.
Filename: "{app}\fotaro.exe"; Description: "{cm:LaunchProgram,Fotaro}"; Flags: nowait postinstall skipifsilent
Filename: "{app}\fotaro.exe"; Flags: nowait skipifnotsilent
