; Inno Setup 6 script: wraps the staged package folder in a standard Windows
; setup wizard (Vernon-<version>-win64-setup.exe).
;
; Build after scripts\release\package.ps1 has produced dist\Vernon-<version>-win64\:
;   ISCC.exe /DAppVersion=0.1.0 /DBuildTag=0.1.0+01183a34b packaging\installer\vernon.iss
;
; Per-user install (no administrator rights), same location, Start Menu entry
; and .vrn file type as install.ps1. Uninstall goes through Settings > Apps.

#ifndef AppVersion
  #define AppVersion "0.1.0"
#endif
#ifndef BuildTag
  #define BuildTag AppVersion
#endif
#define PkgName "Vernon-" + AppVersion + "-win64"
#define PkgDir "..\..\dist\" + PkgName

[Setup]
AppId={{6C3F2A0E-8B9D-4E5A-9C71-3D2B8F4A1E60}
AppName=Vernon
AppVersion={#AppVersion}
AppVerName=Vernon {#BuildTag}
AppPublisher=The Vernon contributors
AppPublisherURL=https://github.com/ssnfts/Vernon
VersionInfoVersion={#AppVersion}
DefaultDirName={localappdata}\Programs\Vernon
DefaultGroupName=Vernon
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
LicenseFile={#PkgDir}\LICENSE
OutputDir=..\..\dist
OutputBaseFilename={#PkgName}-setup
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
ChangesAssociations=yes
ChangesEnvironment=yes
UninstallDisplayIcon={app}\bin\vernon.exe
UninstallDisplayName=Vernon {#AppVersion}

[Tasks]
Name: "desktopicon"; Description: "Create a &desktop shortcut"; GroupDescription: "Shortcuts:"; Flags: unchecked
Name: "vrnassoc"; Description: "Open &.vrn project files with Vernon"; GroupDescription: "Integration:"
Name: "addtopath"; Description: "Add vernon-cli to my &PATH (for terminals)"; GroupDescription: "Integration:"; Flags: unchecked

[Files]
; The zip's script installer is not needed here; Inno provides install and uninstall.
Source: "{#PkgDir}\*"; DestDir: "{app}"; Excludes: "install.cmd,install.ps1,uninstall.cmd,uninstall.ps1,\setup,install.json,portable.txt"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{autoprograms}\Vernon"; Filename: "{app}\bin\vernon.exe"; WorkingDir: "{app}"; Comment: "Vernon node-graph 3D toolkit"
Name: "{autodesktop}\Vernon"; Filename: "{app}\bin\vernon.exe"; WorkingDir: "{app}"; Tasks: desktopicon

[Registry]
Root: HKA; Subkey: "Software\Classes\.vrn"; ValueType: string; ValueName: ""; ValueData: "Vernon.Project"; Flags: uninsdeletevalue uninsdeletekeyifempty; Tasks: vrnassoc
Root: HKA; Subkey: "Software\Classes\Vernon.Project"; ValueType: string; ValueName: ""; ValueData: "Vernon project"; Flags: uninsdeletekey; Tasks: vrnassoc
Root: HKA; Subkey: "Software\Classes\Vernon.Project\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: """{app}\bin\vernon.exe"",0"; Tasks: vrnassoc
Root: HKA; Subkey: "Software\Classes\Vernon.Project\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\bin\vernon.exe"" ""%1"""; Tasks: vrnassoc

[Run]
Filename: "{app}\bin\vernon.exe"; Description: "Launch Vernon"; WorkingDir: "{app}"; Flags: nowait postinstall skipifsilent

[Code]
const
  EnvKey = 'Environment';

function PathRoot: Integer;
begin
  if IsAdminInstallMode then Result := HKEY_LOCAL_MACHINE else Result := HKEY_CURRENT_USER;
end;

function PathKey: String;
begin
  if IsAdminInstallMode then
    Result := 'SYSTEM\CurrentControlSet\Control\Session Manager\Environment'
  else
    Result := EnvKey;
end;

function HasPathEntry(const Paths, Dir: String): Boolean;
begin
  Result := Pos(';' + Uppercase(Dir) + ';', ';' + Uppercase(Paths) + ';') > 0;
end;

procedure AddBinToPath;
var
  Paths, Dir: String;
begin
  Dir := ExpandConstant('{app}\bin');
  if not RegQueryStringValue(PathRoot, PathKey, 'Path', Paths) then Paths := '';
  if HasPathEntry(Paths, Dir) then exit;
  if (Paths <> '') and (Copy(Paths, Length(Paths), 1) <> ';') then Paths := Paths + ';';
  RegWriteExpandStringValue(PathRoot, PathKey, 'Path', Paths + Dir);
end;

procedure RemoveBinFromPath;
var
  Paths, Dir: String;
  P: Integer;
begin
  Dir := ExpandConstant('{app}\bin');
  if not RegQueryStringValue(PathRoot, PathKey, 'Path', Paths) then exit;
  Paths := ';' + Paths + ';';
  P := Pos(';' + Uppercase(Dir) + ';', Uppercase(Paths));
  if P = 0 then exit;
  Delete(Paths, P, Length(Dir) + 1);
  Paths := Copy(Paths, 2, Length(Paths) - 2);
  RegWriteExpandStringValue(PathRoot, PathKey, 'Path', Paths);
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if (CurStep = ssPostInstall) and WizardIsTaskSelected('addtopath') then AddBinToPath;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then RemoveBinFromPath;
end;
