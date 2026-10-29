; Copyright (C) 2026 Samrat Dutta
; SPDX-License-Identifier: GPL-3.0-or-later
; This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

; Lemon Studio installer (Inno Setup). Compiled by build.sh / build.bat beside it, which pass AppVersion, Commit, Stage, Repo,
; OutDir and WithPlugins.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef Commit
  #define Commit "local"
#endif
#ifndef WithPlugins
  #define WithPlugins 1
#endif

[Setup]
AppId={{6F3C2A8E-4B1D-4E7A-9C55-2D8B7E1A4F90}
AppName=Lemon Studio
AppVersion={#AppVersion}+{#Commit}
AppVerName=Lemon Studio {#AppVersion} ({#Commit})
AppPublisher=ItsAlphaNeon
AppPublisherURL=https://github.com/ItsAlphaNeon/Lemon-Studio
DefaultDirName={autopf}\Lemon Studio
DefaultGroupName=Lemon Studio
DisableProgramGroupPage=yes
; Installs per user without admin rights by default; the dialog offers an all-users install.
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
LicenseFile={#Stage}\app\LICENSE.txt
SetupIconFile={#Repo}\crates\os-app\assets\lemonstudio.ico
UninstallDisplayIcon={app}\lemonstudio.exe
OutputDir={#OutDir}
OutputBaseFilename=LemonStudio-Setup-{#AppVersion}-{#Commit}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes

[Types]
Name: "full"; Description: "Lemon Studio and its plugins for other DAWs"
Name: "app"; Description: "Lemon Studio only"
Name: "custom"; Description: "Custom"; Flags: iscustom

[Components]
Name: "app"; Description: "Lemon Studio"; Types: full app custom; Flags: fixed
#if WithPlugins
Name: "plugins"; Description: "Lemon Studio plugins for other DAWs"; Types: full
Name: "plugins\vst3"; Description: "VST3"; Types: full
Name: "plugins\clap"; Description: "CLAP"; Types: full
#endif

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#Stage}\app\*"; DestDir: "{app}"; Components: app; Flags: ignoreversion
#if WithPlugins
; {autocf} is %COMMONPROGRAMFILES% for an all-users install, %LOCALAPPDATA%\Programs\Common for a per-user one:
; the folders DAWs search for VST3 and CLAP plugins.
Source: "{#Stage}\VST3\*"; DestDir: "{autocf}\VST3"; Components: plugins\vst3; Flags: ignoreversion recursesubdirs createallsubdirs skipifsourcedoesntexist
Source: "{#Stage}\CLAP\*"; DestDir: "{autocf}\CLAP"; Components: plugins\clap; Flags: ignoreversion recursesubdirs createallsubdirs skipifsourcedoesntexist
#endif

[Icons]
Name: "{autoprograms}\Lemon Studio"; Filename: "{app}\lemonstudio.exe"
Name: "{autodesktop}\Lemon Studio"; Filename: "{app}\lemonstudio.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\lemonstudio.exe"; Description: "{cm:LaunchProgram,Lemon Studio}"; Flags: nowait postinstall skipifsilent
