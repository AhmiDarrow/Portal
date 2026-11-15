; Portal installer. Build it with installer\build.ps1, which stages the files
; and generates the pinned download list this script includes.
;
; The installer carries only Portal. During setup it downloads, verifies
; (SHA-256) and unpacks what Portal needs: ffmpeg, Kodi when it is missing,
; and a private Python for TV casting when that option is selected.
; No administrator rights are needed, except for Kodi's own installer.

#ifndef AppVersion
  #error Build with installer\build.ps1
#endif
#include Downloads

#define AppName "Portal"
#define Launcher "portal-launcher.exe"
#define RepoUrl "https://github.com/AhmiDarrow/Portal"

[Setup]
AppId={{3C2E0C9D-CA46-4F5A-AD14-AEC1527BF1F4}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher={#AppName}
AppPublisherURL={#RepoUrl}
AppSupportURL={#RepoUrl}/issues
AppUpdatesURL={#RepoUrl}/releases
DefaultDirName={autopf}\{#AppName}
DisableDirPage=auto
DisableProgramGroupPage=yes
DisableWelcomePage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=commandline
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
OutputBaseFilename=Portal-Setup-{#AppVersion}
SetupIconFile={#RepoDir}\assets\portal.ico
UninstallDisplayIcon={app}\{#Launcher}
UninstallDisplayName={#AppName}
WizardStyle=modern
WizardSizePercent=100
Compression=lzma2/max
SolidCompression=yes
ArchiveExtraction=full
CloseApplications=yes
RestartApplications=no
VersionInfoVersion={#AppVersion}
VersionInfoProductName={#AppName}
VersionInfoDescription={#AppName} Setup

[Tasks]
Name: "desktopicon"; Description: "Put a Portal shortcut on the desktop"; GroupDescription: "Shortcuts:"
Name: "startup"; Description: "Start Portal in the tray when I sign in"; GroupDescription: "Shortcuts:"; Flags: unchecked
Name: "kodi"; Description: "Install Kodi {#KodiVersion} (Portal needs it; about {#KodiMB} MB download)"; GroupDescription: "Kodi was not found on this PC:"; Check: KodiMissing and not IsUpdate
Name: "cast"; Description: "Cast to TVs on my network (about {#CastMB} MB download)"; GroupDescription: "Optional:"

[Files]
Source: "{#StageDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#Launcher}"; Comment: "Open Portal"
; An update keeps whatever the user did since: a deleted shortcut stays deleted.
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#Launcher}"; Comment: "Open Portal"; Tasks: desktopicon; Check: not IsUpdate

[InstallDelete]
; Earlier versions had these. Portal's tray icon now quits Portal, and
; starting with Windows is the Run value below.
Type: files; Name: "{autoprograms}\Stop {#AppName}.lnk"
Type: files; Name: "{userstartup}\{#AppName}.lnk"
; The dashboard's file names change with every build, so old ones would pile up.
Type: filesandordirs; Name: "{app}\web"

[Registry]
; The same value the tray's "Start with Windows" item turns on and off. An
; update leaves it alone, so turning it off in the tray sticks.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "{#AppName}"; ValueData: """{app}\{#Launcher}"" --no-browser"; Flags: uninsdeletevalue; Tasks: startup; Check: not IsUpdate

[Run]
Filename: "{app}\{#Launcher}"; Description: "Open Portal now"; Flags: postinstall nowait skipifsilent; Check: not IsUpdate
; A silent install (an update from Portal, or a first install) starts Portal
; in the tray from CurStepChanged, once ffmpeg and Kodi are in place.

[UninstallRun]
Filename: "{app}\{#Launcher}"; Parameters: "--stop --quiet"; Flags: runhidden waituntilterminated; RunOnceId: "StopPortal"

[UninstallDelete]
; Downloaded during setup, so not in the uninstall log.
Type: filesandordirs; Name: "{app}\ffmpeg"
Type: filesandordirs; Name: "{app}\python"
Type: dirifempty; Name: "{app}"

[Code]
const
  FfmpegArchive = 'ffmpeg.7z';
  KodiSetup = 'kodi-setup.exe';
  PythonArchive = 'python-embed.zip';
  DownloadTries = 3;

var
  DownloadPage: TDownloadWizardPage;
  Problems: String;
  QueuedUrls, QueuedNames, QueuedShas: TArrayOfString;
  CastPackages: Integer;

{ Portal's updater runs the new installer with /UPDATE=1. }
function IsUpdate(): Boolean;
begin
  Result := ExpandConstant('{param:UPDATE|0}') = '1';
end;

function KodiDir(): String;
var
  Dir: String;
begin
  Result := '';
  if RegQueryStringValue(HKCU, 'Software\Kodi', '', Dir) and FileExists(AddBackslash(Dir) + 'kodi.exe') then
    Result := Dir
  else if RegQueryStringValue(HKLM, 'Software\Kodi', '', Dir) and FileExists(AddBackslash(Dir) + 'kodi.exe') then
    Result := Dir
  else if FileExists(ExpandConstant('{commonpf64}\Kodi\kodi.exe')) then
    Result := ExpandConstant('{commonpf64}\Kodi')
  else if FileExists(ExpandConstant('{commonpf32}\Kodi\kodi.exe')) then
    Result := ExpandConstant('{commonpf32}\Kodi');
end;

function KodiMissing(): Boolean;
begin
  Result := KodiDir() = '';
end;

{ A folder unpacked by an earlier setup is reused when its VERSION.txt matches. }
function Unpacked(const Dir, Id: String): Boolean;
var
  Text: AnsiString;
begin
  Result := LoadStringFromFile(AddBackslash(Dir) + 'VERSION.txt', Text) and (Trim(String(Text)) = Id);
end;

function NeedFfmpeg(): Boolean;
begin
  Result := not Unpacked(ExpandConstant('{app}\ffmpeg'), '{#FfmpegVersion}');
end;

function NeedCast(): Boolean;
begin
  Result := WizardIsTaskSelected('cast') and not Unpacked(ExpandConstant('{app}\python'), '{#CastRuntimeId}');
end;

function NeedKodi(): Boolean;
begin
  Result := WizardIsTaskSelected('kodi') and KodiMissing();
end;

procedure Problem(const Text: String);
begin
  Log('Problem: ' + Text);
  Problems := Problems + #13#10 + '- ' + Text;
end;

procedure Queue(const Url, Name, Sha: String);
var
  Count: Integer;
begin
  Count := GetArrayLength(QueuedUrls);
  SetArrayLength(QueuedUrls, Count + 1);
  SetArrayLength(QueuedNames, Count + 1);
  SetArrayLength(QueuedShas, Count + 1);
  QueuedUrls[Count] := Url;
  QueuedNames[Count] := Name;
  QueuedShas[Count] := Sha;
end;

{ CastWheels is "url|sha256|url|sha256|..." }
procedure AddWheels();
var
  Rest, Url, Sha: String;
  Cut, Index: Integer;
begin
  Rest := '{#CastWheels}';
  Index := 0;
  while Rest <> '' do begin
    Cut := Pos('|', Rest);
    Url := Copy(Rest, 1, Cut - 1);
    Delete(Rest, 1, Cut);
    Cut := Pos('|', Rest);
    if Cut = 0 then begin
      Sha := Rest;
      Rest := '';
    end else begin
      Sha := Copy(Rest, 1, Cut - 1);
      Delete(Rest, 1, Cut);
    end;
    Index := Index + 1;
    Queue(Url, Format('cast-package-%d.zip', [Index]), Sha);
  end;
  CastPackages := Index;
end;

procedure InitializeWizard();
begin
  DownloadPage := CreateDownloadPage('Downloading', 'Setup is downloading what Portal needs. This only happens once.', nil);
  DownloadPage.ShowBaseNameInsteadOfUrl := True;
end;

function UpdateReadyMemo(Space, NewLine, MemoUserInfoInfo, MemoDirInfo, MemoTypeInfo,
  MemoComponentsInfo, MemoGroupInfo, MemoTasksInfo: String): String;
var
  Items: String;
begin
  Result := MemoDirInfo;
  if MemoTasksInfo <> '' then
    Result := Result + NewLine + NewLine + MemoTasksInfo;
  if NeedFfmpeg() then
    Items := Items + Space + 'ffmpeg {#FfmpegVersion} (about {#FfmpegMB} MB)' + NewLine;
  if NeedKodi() then
    Items := Items + Space + 'Kodi {#KodiVersion} (about {#KodiMB} MB)' + NewLine;
  if NeedCast() then
    Items := Items + Space + 'Python for casting (about {#CastMB} MB)' + NewLine;
  if Items <> '' then
    Result := Result + NewLine + NewLine + 'Setup will download:' + NewLine + Items;
end;

{ One queued file, tried again after a failure. Aborted is set when the
  user stopped the downloads. }
function DownloadOne(Index: Integer; var Aborted: Boolean): Boolean;
var
  Attempt: Integer;
begin
  Result := False;
  for Attempt := 1 to DownloadTries do begin
    DownloadPage.Clear;
    DownloadPage.Add(QueuedUrls[Index], QueuedNames[Index], QueuedShas[Index]);
    try
      DownloadPage.Download;
      Result := True;
      exit;
    except
      if DownloadPage.AbortedByUser then begin
        Aborted := True;
        exit;
      end;
      Log(Format('Download of %s failed (try %d of %d): %s', [QueuedNames[Index], Attempt, DownloadTries, GetExceptionMessage]));
    end;
  end;
end;

{ Each file is fetched on its own, so one that times out does not cost the
  ones queued after it. }
function NextButtonClick(CurPageID: Integer): Boolean;
var
  Index: Integer;
  Aborted: Boolean;
  Failed: String;
begin
  Result := True;
  if CurPageID <> wpReady then
    exit;
  SetArrayLength(QueuedUrls, 0);
  SetArrayLength(QueuedNames, 0);
  SetArrayLength(QueuedShas, 0);
  if NeedFfmpeg() then
    Queue('{#FfmpegUrl}', FfmpegArchive, '{#FfmpegSha256}');
  if NeedKodi() then
    Queue('{#KodiUrl}', KodiSetup, '{#KodiSha256}');
  if NeedCast() then begin
    Queue('{#PythonUrl}', PythonArchive, '{#PythonSha256}');
    AddWheels();
  end;
  if GetArrayLength(QueuedUrls) = 0 then
    exit;
  Aborted := False;
  Failed := '';
  DownloadPage.Show;
  try
    for Index := 0 to GetArrayLength(QueuedUrls) - 1 do
      if not DownloadOne(Index, Aborted) then begin
        if Aborted then begin
          Result := False;
          exit;
        end;
        Failed := Failed + #13#10 + '- ' + QueuedNames[Index];
      end;
  finally
    DownloadPage.Hide;
  end;
  if Failed <> '' then
    Result := SuppressibleMsgBox(
      'These downloads failed:' + Failed + #13#10#13#10 +
      'Install Portal anyway? Run this installer again later to finish the missing parts.',
      mbError, MB_YESNO, IDYES) = IDYES;
end;

{ Ends the tray icon and service run from the install folder, the tray icon
  first so it cannot start the service again. A Portal run from anywhere
  else, such as a developer build, is left alone. }
procedure EndInstalledProcesses();
var
  Code: Integer;
  Dir: String;
begin
  Dir := AddBackslash(ExpandConstant('{app}'));
  StringChangeEx(Dir, '''', '''''', True);
  Exec(ExpandConstant('{sys}\WindowsPowerShell\v1.0\powershell.exe'),
    '-NoProfile -NonInteractive -Command "''portal-launcher'',''portal'' | ' +
    'ForEach-Object { Get-Process -Name $_ -ErrorAction SilentlyContinue } | ' +
    'Where-Object { $_.Path -and $_.Path.StartsWith(''' + Dir + ''', ''OrdinalIgnoreCase'') } | ' +
    'Stop-Process -Force -PassThru | Wait-Process -Timeout 10 -ErrorAction SilentlyContinue"',
    '', SW_HIDE, ewWaitUntilTerminated, Code);
end;

{ Stops Portal's tray icon and service so their files can be replaced. }
function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  Code: Integer;
  Launcher: String;
begin
  Result := '';
  Launcher := ExpandConstant('{app}\{#Launcher}');
  if not FileExists(Launcher) then
    exit;
  if Exec(Launcher, '--stop --quiet', '', SW_HIDE, ewWaitUntilTerminated, Code) and (Code = 0) then
    exit;
  { The polite stop failed; its files are about to be replaced, so end it. }
  Log('Portal did not stop cleanly; ending its processes.');
  EndInstalledProcesses();
end;

procedure Status(const Text: String);
begin
  WizardForm.StatusLabel.Caption := Text;
  WizardForm.FilenameLabel.Caption := '';
end;

function FirstSubdir(const Dir: String): String;
var
  Find: TFindRec;
begin
  Result := '';
  if FindFirst(AddBackslash(Dir) + '*', Find) then
    try
      repeat
        if ((Find.Attributes and FILE_ATTRIBUTE_DIRECTORY) <> 0) and (Find.Name <> '.') and (Find.Name <> '..') then begin
          Result := AddBackslash(Dir) + Find.Name;
          exit;
        end;
      until not FindNext(Find);
    finally
      FindClose(Find);
    end;
end;

procedure SetUpFfmpeg();
var
  Archive, Scratch, Build, Dest: String;
begin
  Archive := ExpandConstant('{tmp}\' + FfmpegArchive);
  if not FileExists(Archive) then begin
    if NeedFfmpeg() then
      Problem('ffmpeg was not downloaded. Video playback needs it.');
    exit;
  end;
  Status('Setting up ffmpeg...');
  Scratch := ExpandConstant('{tmp}\ffmpeg');
  Dest := ExpandConstant('{app}\ffmpeg');
  try
    ExtractArchive(Archive, Scratch, '', True, nil);
    Build := FirstSubdir(Scratch);
    DelTree(Dest, True, True, True);
    ForceDirectories(Dest);
    if not (FileCopy(Build + '\bin\ffmpeg.exe', Dest + '\ffmpeg.exe', False)
      and FileCopy(Build + '\bin\ffprobe.exe', Dest + '\ffprobe.exe', False)) then
      RaiseException('the archive layout was not recognised');
    FileCopy(Build + '\LICENSE', Dest + '\LICENSE.txt', False);
    SaveStringToFile(Dest + '\README.txt',
      'Unmodified ffmpeg {#FfmpegVersion} "essentials" build from https://www.gyan.dev/ffmpeg/builds/' + #13#10 +
      'Licensed under the GNU GPL version 3 (LICENSE.txt). Source: https://ffmpeg.org/download.html' + #13#10 +
      'Portal runs ffmpeg and ffprobe as separate programs.' + #13#10, False);
    SaveStringToFile(Dest + '\VERSION.txt', '{#FfmpegVersion}', False);
  except
    Problem('ffmpeg could not be set up: ' + GetExceptionMessage);
  end;
  DelTree(Scratch, True, True, True);
end;

procedure SetUpCast();
var
  Archive, Dest, Site, Pth: String;
  Find: TFindRec;
  Index: Integer;
begin
  Archive := ExpandConstant('{tmp}\' + PythonArchive);
  if not FileExists(Archive) then begin
    if NeedCast() then
      Problem('The casting runtime was not downloaded. Casting to TVs will not work.');
    exit;
  end;
  { Each package has its own download now. A runtime missing one would not
    cast, and its VERSION.txt would stop the next setup from repairing it,
    so a working runtime is only replaced by a complete one. }
  for Index := 1 to CastPackages do
    if not FileExists(ExpandConstant(Format('{tmp}\cast-package-%d.zip', [Index]))) then begin
      Problem('Part of the casting runtime was not downloaded. Casting to TVs will not work.');
      exit;
    end;
  Status('Setting up casting...');
  Dest := ExpandConstant('{app}\python');
  Site := Dest + '\Lib\site-packages';
  try
    DelTree(Dest, True, True, True);
    ExtractArchive(Archive, Dest, '', True, nil);
    ForceDirectories(Site);
    Index := 1;
    while FileExists(ExpandConstant(Format('{tmp}\cast-package-%d.zip', [Index]))) do begin
      ExtractArchive(ExpandConstant(Format('{tmp}\cast-package-%d.zip', [Index])), Site, '', True, nil);
      Index := Index + 1;
    end;
    { The embeddable runtime only searches the folders its ._pth file lists. }
    if FindFirst(Dest + '\python*._pth', Find) then
      try
        Pth := Dest + '\' + Find.Name;
      finally
        FindClose(Find);
      end;
    if (Pth = '') or not SaveStringToFile(Pth, #13#10 + 'Lib\site-packages' + #13#10, True) then
      RaiseException('the Python runtime layout was not recognised');
    SaveStringToFile(Dest + '\VERSION.txt', '{#CastRuntimeId}', False);
  except
    Problem('Casting could not be set up: ' + GetExceptionMessage);
    DelTree(Dest, True, True, True);
  end;
end;

procedure SetUpKodi();
var
  Setup: String;
  Code: Integer;
begin
  Setup := ExpandConstant('{tmp}\' + KodiSetup);
  if not FileExists(Setup) then begin
    if NeedKodi() then
      Problem('Kodi was not downloaded. Install it from https://kodi.tv/download before opening Portal.');
    exit;
  end;
  Status('Installing Kodi {#KodiVersion}... (Windows may ask for permission)');
  { Kodi's installer asks for administrator rights itself. }
  if not ShellExec('', Setup, '/S', '', SW_SHOWNORMAL, ewWaitUntilTerminated, Code) or KodiMissing() then
    Problem('Kodi was not installed. Install it from https://kodi.tv/download before opening Portal.');
end;

procedure CurStepChanged(CurStep: TSetupStep);
var
  Code: Integer;
begin
  if CurStep = ssPostInstall then begin
    SetUpFfmpeg();
    SetUpCast();
    SetUpKodi();
  end else if CurStep = ssDone then begin
    if Problems <> '' then
      SuppressibleMsgBox('Portal is installed, with some problems:' + #13#10 + Problems, mbInformation, MB_OK, IDOK);
    { [Run] entries run before ssPostInstall, which is too early: Portal
      would start before ffmpeg is unpacked and Kodi is installed. }
    if WizardSilent or IsUpdate then
      Exec(ExpandConstant('{app}\{#Launcher}'), '--no-browser', '', SW_SHOWNORMAL, ewNoWait, Code);
  end;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Data: String;
  Code: Integer;
begin
  if CurUninstallStep = usUninstall then begin
    { The tray's "Start with Windows" may have set this without the installer. }
    RegDeleteValue(HKCU, 'Software\Microsoft\Windows\CurrentVersion\Run', '{#AppName}');
    { A Portal that does not stop politely would keep its files in use. }
    if not Exec(ExpandConstant('{app}\{#Launcher}'), '--stop --quiet', '', SW_HIDE, ewWaitUntilTerminated, Code)
      or (Code <> 0) then
      EndInstalledProcesses();
  end;
  if CurUninstallStep <> usPostUninstall then
    exit;
  { Caches, downloads and logs go either way; the stream cache alone can be gigabytes. }
  Data := ExpandConstant('{localappdata}\Portal');
  DelTree(Data + '\hls', True, True, True);
  DelTree(Data + '\updates', True, True, True);
  DelTree(Data + '\staging', True, True, True);
  DelTree(Data + '\logs', True, True, True);
  DelTree(Data + '\subtitles', True, True, True);
  DeleteFile(Data + '\metadata-cache.json');
  if UninstallSilent() then
    exit;
  if MsgBox('Also delete your Portal library (watch history, watchlist, bookmarks and Portal settings)?' + #13#10#13#10 +
    'Kodi and its add-ons are not affected either way.', mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES then
    DelTree(Data, True, True, True);
end;
