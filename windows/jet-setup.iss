; Inno Setup — the jet GUI installer (a Next -> Next -> Finish wizard).
; Installs jet.exe into %LOCALAPPDATA%\Programs\jet (no administrator rights)
; and adds the folder to the user PATH. Creates an uninstaller.
;
; Compilation:
;   1) Install Inno Setup 6:  https://jrsoftware.org/isdl.php
;   2) Put the built binary next to it as jet.exe (rename jet-windows-x64.exe -> jet.exe)
;   3) Run:  "C:\Program Files (x86)\Inno Setup 6\ISCC.exe" jet-setup.iss
;   -> you get Output\jet-setup.exe
;
; The version can be passed at build time:  ISCC.exe /DAppVersion=0.1.0 jet-setup.iss

#ifndef AppVersion
  #define AppVersion "0.1.0"
#endif
#define AppName "jet"
#define AppPublisher "jet"
#define AppURL "https://jet.kim"

[Setup]
AppId={{A1F5C3E2-7B94-4D6A-9E18-2C7F0B3A6D51}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher={#AppPublisher}
AppPublisherURL={#AppURL}
AppSupportURL={#AppURL}
DefaultDirName={localappdata}\Programs\jet
DefaultGroupName=jet
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
OutputBaseFilename=jet-setup
OutputDir=Output
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
ChangesEnvironment=yes
ArchitecturesInstallIn64BitMode=x64
UninstallDisplayName=jet {#AppVersion}
UninstallDisplayIcon={app}\jet.exe

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Files]
; jet.exe is expected next to this script.
Source: "jet.exe"; DestDir: "{app}"; Flags: ignoreversion

[Tasks]
Name: "addtopath"; Description: "Add jet to PATH (recommended)"; GroupDescription: "Integration:"

[Registry]
; Add the install folder to the user PATH (only if it isn't already there).
Root: HKCU; Subkey: "Environment"; ValueType: expandsz; ValueName: "Path"; \
  ValueData: "{olddata};{app}"; Check: NeedsAddPath('{app}'); Tasks: addtopath

[Icons]
Name: "{group}\jet (help)"; Filename: "{cmd}"; Parameters: "/k ""{app}\jet.exe"" help"
Name: "{group}\Uninstall jet"; Filename: "{uninstallexe}"

[Run]
Filename: "{cmd}"; Parameters: "/k ""{app}\jet.exe"" help"; \
  Description: "Open jet and show the commands"; Flags: postinstall skipifsilent

[Code]
function NeedsAddPath(Param: string): Boolean;
var
  OrigPath: string;
begin
  if not RegQueryStringValue(HKEY_CURRENT_USER, 'Environment', 'Path', OrigPath) then
  begin
    Result := True;
    exit;
  end;
  { already on PATH? then don't duplicate }
  Result := Pos(';' + ExpandConstant(Param) + ';', ';' + OrigPath + ';') = 0;
end;

{ Remove our folder from PATH on uninstall. }
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  OrigPath, AppDir, Cleaned: string;
  P: Integer;
begin
  if CurUninstallStep <> usUninstall then
    exit;
  if not RegQueryStringValue(HKEY_CURRENT_USER, 'Environment', 'Path', OrigPath) then
    exit;
  AppDir := ExpandConstant('{app}');
  Cleaned := ';' + OrigPath + ';';
  P := Pos(';' + AppDir + ';', Cleaned);
  if P > 0 then
  begin
    Delete(Cleaned, P, Length(AppDir) + 1);
    { strip the surrounding ';' }
    if (Length(Cleaned) > 0) and (Cleaned[1] = ';') then Delete(Cleaned, 1, 1);
    if (Length(Cleaned) > 0) and (Cleaned[Length(Cleaned)] = ';') then Delete(Cleaned, Length(Cleaned), 1);
    RegWriteExpandStringValue(HKEY_CURRENT_USER, 'Environment', 'Path', Cleaned);
  end;
end;
