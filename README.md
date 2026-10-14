# get_dump_full —— Lertaro 诊断包收集工具

一个单文件 Windows 程序，用于在 Lertaro 崩溃、闪退或功能异常后，把排查所需的资料一次性收集到一个 ZIP 里，交给开发者分析。

全程在本机完成：**程序不联网、不上传任何数据**，ZIP 是否提交由用户自己决定。

## 怎么用

1. 右键“以管理员身份运行”，或直接双击（程序通过 UAC 申请一次管理员权限；读取服务日志与系统账户的崩溃目录、抓取其他进程的内存都需要它）。
2. 等待命令行窗口结束（无参数运行时按回车关闭窗口）。
3. 取走程序同级目录下 `dump_file\<时间戳>-<进程ID>.zip`。

传入 `--help` / `-h` / `help` / `extract` / `--extract` 或任意其他参数时，只打印版本号、不做采集（帮助类参数会把内嵌的 `main.rs` 源码写到 `dump_file\` 下）。

构建：`cargo build --release`（需要 MSVC 工具链，产物为 `target\release\get_dump_full.exe`）。

## 执行了什么操作

按代码执行顺序：

| 步骤 | 外部程序 | 命令 | 目的 |
| --- | --- | --- | --- |
| 1 | `.dump_tools\fastfetch.exe` | `--logo None` | 采集系统与硬件信息 → `system\fastfetch.txt` |
| 2 | `reg.exe` | `query <Uninstall 根> /s`（HKLM 64 位、HKLM WOW6432Node、HKCU 三处） | 由 `DisplayName` 含 Lertaro 的卸载项读 `InstallLocation`/`Inno Setup: App Path`，定位安装目录 |
| 3 | `sc.exe` | `qc LertaroService` | 读 `BINARY_PATH_NAME`，得到后台服务真正运行的安装目录 |
| 4 | `icacls.exe` | 逐个目录名 | 采集安装目录与配置目录的 NTFS 权限 → `system\icacls.txt` |
| 5 | `whoami.exe` | `/user`、`/groups`、`/priv` | 采集账户、令牌组与完整性级别、权限列表 |
| 6 | `reg.exe` | `query HKLM\...\CurrentVersion\Policies\System` | 读 UAC 策略值（`EnableLUA` 等），判断账户能否请求管理员 → `system\account.txt` |
| 7 | `reg.exe` + 目录扫描 | `query HKLM\...\NET Framework Setup\NDP\v4\Full`、`v4\Client`；列 `%ProgramFiles%\dotnet` 与 `%ProgramFiles(x86)%\dotnet` 下的版本目录 | 采集 .NET Framework 与 .NET 7/8/9/10 安装情况 → `system\dotnet.txt` |
| 8 | `tasklist.exe` | `/FO CSV /NH` | 判断 `lertaro.app.exe`、`lertaro.service.exe` 是否在运行，决定走哪条采集分支 |
| 9a | `.dump_tools\procdump.exe` | `-accepteula -ma -o <PID> PROCESSNAME_PID.dmp` | **进程仍在运行时**：抓取完整内存转储，文件名里的 `PROCESSNAME`、`PID` 由 Procdump 替换为 `<进程名>_<进程ID>.dmp` |
| 9b | `wevtutil.exe` | `qe Application/System /rd:true /f:RenderedXml /e:Events /uni:true /q:<XPath>` | **进程未运行时**：取最近 24 小时、文本中提到 Lertaro 的错误事件 → `crash\events\*.xml` |
| 10 | 自身文件复制 | — | 日志、WER 崩溃报告、系统 CrashDumps 目录中的 Lertaro dump |
| 11 | `.dump_tools\7z.exe` | `a -tzip -mx=0 ...` | 打包 ZIP（`-mx=0` 为不压缩，避免大 dump 反复压缩耗时） |

第 1–7 步在两种分支下都会执行。

内嵌的 fastfetch / procdump / 7-Zip 在运行时解压到 `dump_file\.dump_tools\`（与采集目录同级，不会被写进 ZIP），运行结束后**不会自动删除**，可自行清理。

单个步骤失败只记录到 `run.log`，不影响其余采集与打包。

## 控制台输出

每一步打印一行状态，并在运行期间每秒刷新自己的耗时，避免长时间看起来像卡住：

```
正在采集系统信息（0 min 2 s）
正在读取目录权限（0 min 3 s）
正在检查账户权限（0 min 1 s）
正在检查 .NET 环境（0 min 1 s）
正在检查 Lertaro 进程（0 min 1 s）
正在转储 lertaro.app.exe (PID 19252)（0 min 12 s）      ← 仅目标进程在运行时
正在复制应用日志（0 min 1 s）                            ← 仅目标进程未运行时
正在查询事件日志（0 min 4 s）
正在收集崩溃报告（0 min 1 s）
正在收集崩溃转储（0 min 0 s）
正在压缩诊断文件（1 min 20 s）
诊断包已生成：dump_file\2026-10-14_20-24-21-723-22052.zip
```

同一行原地刷新，结束时保留最终耗时并换行。失败信息仍然走标准错误，格式是 `[时间][ERROR][模块]: 上下文: 原因`，同时写进 `run.log`。

## 会获取用户的哪些信息

### 1. 系统与硬件信息（`system\fastfetch.txt`）

计算机名、当前用户名、操作系统名称与版本、内核版本、主板/机型、CPU 型号与核心数、显卡型号与显存、内存与页面文件占用、每个磁盘盘符的容量与剩余空间及文件系统、显示器型号/分辨率/刷新率、主题与字体设置、终端、包管理器统计、本地 IP 地址（含虚拟网卡）、系统区域设置、开机时长。

### 2. 目录权限（`system\icacls.txt`）

对以下目录执行 `icacls`（只看目录本身，不递归），输出会包含账户名与安全标识符（SID）：

- 安装目录：注册表卸载项记录的路径 + 服务映像所在路径（标签 `install-registry`、`install-service`）
- `%LOCALAPPDATA%\Lertaro`（标签 `config-user`）
- `%ProgramData%\Lertaro`（标签 `config-machine`）

目录不存在时只写一行“目录不存在”，不中断。

### 3. 账户与权限（`system\account.txt`）

- `whoami /user`：当前账户名与其 SID
- `whoami /groups`：进程令牌中的组、属性与完整性级别（`Mandatory Label`）。**注意**：用 Microsoft 账户登录时，列表里会有形如 `MicrosoftAccount\你的邮箱` 的声明，邮箱地址因此出现在报告中
- `whoami /priv`：当前进程的权限（Privilege）列表，例如 `SeDebugPrivilege` 决定 Procdump 能否打开其他进程
- UAC 策略（注册表 `HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System`，只读）：`EnableLUA`、`ConsentPromptBehaviorAdmin`、`ConsentPromptBehaviorUser`、`PromptOnSecureDesktop`、`FilterAdministratorToken`、`LocalAccountTokenFilterPolicy`；值不存在记为 `(absent)`
- `[summary]`：由令牌推导的三行结论——`local administrator`（账户是否属于本地管理员组，即能否请求提权）、`integrity`（Low/Medium/High/System 及对应 SID）、`already elevated`（当前进程是否已提权）

判定依据是本地管理员组的固定 SID `S-1-5-32-544` 是否出现在令牌中，不依赖语言版本；本程序正常以管理员身份运行，所以通常是 `High` + `already elevated: yes`。

### 4. .NET 运行环境（`system\dotnet.txt`）

- .NET Framework：读 `HKLM\SOFTWARE\Microsoft\NET Framework Setup\NDP\v4\Full` 与 `v4\Client` 的 `Install`/`Release`/`Version`，按微软公布的 Release 阈值推断已装版本，并逐项给出 `4`、`4.5`、`4.8` 是否存在（阈值推断只说明“至少是这个版本”）
- .NET 7/8/9/10：扫描 `%ProgramFiles%\dotnet`（x64）与 `%ProgramFiles(x86)%\dotnet`（x86）下 `shared\Microsoft.NETCore.App`（运行时）、`shared\Microsoft.WindowsDesktop.App`（WPF 依赖的桌面运行时）、`sdk` 三类版本目录，列出实际版本号并按主版本给出 `7`/`8`/`9`/`10` 清单
- 这一项只记录注册表值与目录名，不读取这些安装目录内的任何文件内容

### 5. 日志文件（只读复制，不移动、不删除、不修改源文件）

- `%LOCALAPPDATA%\Lertaro\logs\**` → `logs\user\`（递归复制全部用户日志；日志内容可能包含访问过的文件名与路径、搜索关键字等）
- `%ProgramData%\Lertaro\logs\service.log` → `logs\service\`

### 6. 崩溃相关

- Windows 事件查看器 Application / System 通道最近 24 小时的错误事件，且事件正文中提到 Lertaro 的才写入 `crash\events\`
- Windows 错误报告（WER）`ReportArchive`、`ReportQueue` 中报告对象为 Lertaro 程序的文件夹 → `crash\wer\<user|machine>\`（内含计算机名、用户名、模块路径等）
- 用户、系统账户（`%SystemRoot%\System32\config\systemprofile`）、LocalService、NetworkService 四个 `AppData\Local\CrashDumps` 目录中，文件名以 `lertaro.app.exe.` 或 `lertaro.service.exe.` 开头且以 `.dmp` 结尾的文件 → `crash\dumps\<user|system|local-service|network-service>\`
- 目标进程在运行时的**完整内存转储**（`-ma`）。内存转储是进程当时内存的原样快照，可能包含剪贴板内容、正在编辑的文本、已登录令牌等敏感数据，请务必确认接收方可信后再分享。

### 7. 其他

`run.log` 记录本次运行中失败的步骤；进程列表本身只用于判断分支，不会整份写入 ZIP。

## 不做什么

- 不读取浏览器数据、凭据管理器与其他应用程序的文件
- 注册表只查询、不写入：枚举卸载项列表以定位安装目录（结果里只保留 Lertaro 相关的路径），另外只读固定的 UAC 策略键与 .NET Framework 安装键
- 不联网，不做任何自动上报
- 不修改、移动或删除任何被采集的文件；写入的只有 `dump_file\` 与 `dump_file\.dump_tools\`
- 不安装服务、不改注册表（只查询）、不改任何目录的权限（`icacls` 仅读取）

## 产物结构

```
<程序所在目录>\
├─ get_dump_full.exe
└─ dump_file\
   ├─ .dump_tools\            解压出的 fastfetch.exe / procdump.exe / 7z.exe
   ├─ 2026-10-14_19-55-47-876-31392\
   │  ├─ run.log              失败步骤记录
   │  ├─ system\fastfetch.txt
   │  ├─ system\icacls.txt
   │  ├─ system\account.txt
   │  ├─ system\dotnet.txt
   │  ├─ logs\user\...        logs\service\service.log
   │  ├─ crash\events\*.xml   crash\wer\... crash\dumps\...
   │  └─ Lertaro.App.exe_<PID>.dmp  仅进程在运行时产生，每个目标进程一份
   └─ 2026-10-14_19-55-47-876-31392.zip
```

每次无参数运行都会新建以“时间戳-进程ID”命名的会话目录，旧会话不会被覆盖，也不会被后一次的 ZIP 收录。

## 第三方组件

程序内嵌以下工具，运行时解压使用，各自许可见 `res\` 下的许可文件：

- 7-Zip `7z.exe` / `7z.dll` —— LGPL 等，见 `res\7z-license.txt`
- fastfetch —— MIT，见 `res\fastfetch-license.txt`
- ProcDump —— Microsoft Sysinternals 软件许可条款

本项目自身采用 MIT 许可，见 `LICENSE`。
