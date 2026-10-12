; Inno Setup 脚本 - 生成 ntfy-windows 安装程序
; 使用方法:
;   1. 先执行: cargo build --release
;   2. 安装 Inno Setup 后运行: iscc packaging\ntfy-windows.iss
;   3. 产物位于 dist\ntfy-windows-setup-<version>.exe

#define MyAppName "ntfy-windows"
#define MyAppVersion "0.1.1"
#define MyAppPublisher "JackA1ltman"
#define MyAppURL "https://github.com/JackA1ltman/ntfy-windows"
#define MyAppExeName "ntfy-windows.exe"
#define SourceExe "..\target\release\ntfy-windows.exe"

[Setup]
AppId={{8F3E1C2A-9B7D-4E5A-9C1D-6E2B4A7F0C31}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppVerName={#MyAppName} {#MyAppVersion}
AppPublisher={#MyAppPublisher}
AppPublisherURL={#MyAppURL}
AppSupportURL={#MyAppURL}
AppUpdatesURL={#MyAppURL}
DefaultDirName={autopf}\{#MyAppName}
DefaultGroupName={#MyAppName}
DisableProgramGroupPage=yes
OutputDir=..\dist
OutputBaseFilename=ntfy-windows-setup-{#MyAppVersion}
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
ArchitecturesInstallIn64BitMode=x64
PrivilegesRequired=lowest
UninstallDisplayName={#MyAppName}
UninstallDisplayIcon={app}\{#MyAppExeName}

[Tasks]
Name: "startupicon"; Description: "开机自动启动（后台运行）"; GroupDescription: "附加任务:"; Flags: checkedonce
Name: "desktopicon"; Description: "创建桌面快捷方式"; GroupDescription: "附加任务:"; Flags: unchecked

[Files]
Source: "{#SourceExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\config.example.json"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\lang\*.toml"; DestDir: "{app}\lang"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion isreadme
Source: "..\README.en.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"
Name: "{group}\卸载 {#MyAppName}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#MyAppExeName}"; Description: "立即启动 {#MyAppName}"; Flags: nowait postinstall skipifsilent

[Registry]
; 选择“开机自动启动”时写入 HKCU Run
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "ntfy-windows"; ValueData: """{app}\{#MyAppExeName}"" --no-gui"; Tasks: startupicon; Flags: uninsdeletevalue

[UninstallDelete]
Type: filesandordirs; Name: "{app}"
