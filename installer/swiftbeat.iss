; Instalador de Swiftbeat (Inno Setup 6).
; Compilar con installer\build.ps1, que pasa la versión de Cargo.toml.

#define AppName "Swiftbeat"
#ifndef AppVersion
  #define AppVersion "0.1.1"
#endif
#define AppExe "swiftbeat.exe"
; Debe coincidir con APP_USER_MODEL_ID en src/main.rs.
#define AppUserModelId "Swiftbeat.Swiftbeat"
#define AppUrl "https://github.com/EliseoUTRNG/Swiftbeat"

[Setup]
AppId={{2285E3FA-25B3-44AF-BAEE-D016F1FB3189}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=Swiftbeat
AppPublisherURL={#AppUrl}
AppSupportURL={#AppUrl}/issues
AppUpdatesURL={#AppUrl}/releases
; Instalación por usuario, sin permisos de administrador (como Spotify).
PrivilegesRequired=lowest
DefaultDirName={localappdata}\Programs\{#AppName}
DisableDirPage=yes
DisableProgramGroupPage=yes
OutputDir=Output
OutputBaseFilename=Swiftbeat-Setup-{#AppVersion}
SetupIconFile=..\assets\swiftbeat.ico
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}
VersionInfoVersion={#AppVersion}
VersionInfoProductName={#AppName}
Compression=lzma2/ultra64
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
CloseApplications=yes
RestartApplications=no

[Languages]
Name: "spanish"; MessagesFile: "compiler:Languages\Spanish.isl"
Name: "english"; MessagesFile: "compiler:Default.isl"

[CustomMessages]
spanish.DeleteUserData=¿Quieres borrar también tu sesión y tus ajustes de Swiftbeat?
english.DeleteUserData=Do you also want to delete your Swiftbeat session and settings?

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"

[Files]
Source: "..\target\release\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"; AppUserModelID: "{#AppUserModelId}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; AppUserModelID: "{#AppUserModelId}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent

[Code]
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  DataDir: String;
begin
  if CurUninstallStep <> usPostUninstall then
    exit;
  DataDir := ExpandConstant('{userappdata}\{#AppName}');
  if UninstallSilent or not DirExists(DataDir) then
    exit;
  if MsgBox(CustomMessage('DeleteUserData'), mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES then
    DelTree(DataDir, True, True, True);
end;
