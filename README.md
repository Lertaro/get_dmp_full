# get_dump_full —— Lertaro 诊断包收集工具

一个单文件 Windows 程序，用于在 Lertaro 崩溃、闪退或功能异常后，把排查所需的资料一次性收集到一个 ZIP 里，交给开发者分析。

全程在本机完成：**程序不联网、不上传任何数据**，ZIP 是否提交由用户自己决定；只有在结尾的提问里回答“是”，它才会请求系统打开上传页面。

## 怎么用

1. 右键“以管理员身份运行”，或直接双击（程序通过 UAC 申请一次管理员权限；读取服务日志与系统账户的崩溃目录、抓取其他进程的内存都需要它）。
2. 开始时回答一个问题：本次是否与内存有关（决定转储类型，直接回车取默认“否”）。只有目标进程正在运行时才会问这个问题，也不运行时不会有转储。结束时再问是否要打开上传站点。
3. 等待命令行窗口结束（无参数运行时按回车关闭窗口）。
4. 取走程序同级目录下 `dump_file\<时间戳>-<进程ID>.zip`。

传入 `--help` / `-h` / `help` / `extract` / `--extract` 或任意其他参数时，只打印版本号、不做采集（帮助类参数会把内嵌的 `main.rs` 源码写到 `dump_file\` 下）。

构建：`cargo build --release`（需要 MSVC 工具链，产物为 `target\release\get_dump_full.exe`）。

## 执行了什么操作

按代码执行顺序：

| 步骤 | 外部程序 | 命令 | 目的 |
| --- | --- | --- | --- |
| 1 | `tasklist.exe` | `/FO CSV /NH` | 判断 `lertaro.app.exe`、`lertaro.service.exe` 是否在运行；在运行才会提问、才会抓转储 |
| 2 | 控制台提问 | — | 询问“本次问题是否与内存有关”，据此选择转储类型（见第 13 步） |
| 3 | `.dump_tools\fastfetch.exe` | `--logo None` | 采集系统与硬件信息 → `system\fastfetch.txt` |
| 4 | `reg.exe` | `query <Uninstall 根> /s`（HKLM 64 位、HKLM WOW6432Node、HKCU 三处） | 由 `DisplayName` 含 Lertaro 的卸载项读 `InstallLocation`/`Inno Setup: App Path`，定位安装目录 |
| 5 | `sc.exe` | `qc LertaroService` | 读 `BINARY_PATH_NAME`，得到后台服务真正运行的安装目录 |
| 6 | `icacls.exe` | 逐个目录名 | 采集安装目录与配置目录的 NTFS 权限 → `system\icacls.txt` |
| 7 | `whoami.exe` | `/user`、`/groups`、`/priv` | 采集账户、令牌组与完整性级别、权限列表 |
| 8 | `reg.exe` | `query HKLM\...\CurrentVersion\Policies\System` | 读 UAC 策略值（`EnableLUA` 等），判断账户能否请求管理员 → `system\account.txt` |
| 9 | `reg.exe` + 目录扫描 | `query HKLM\...\NET Framework Setup\NDP\v4\Full`、`v4\Client`；列 `%ProgramFiles%\dotnet` 与 `%ProgramFiles(x86)%\dotnet` 下的版本目录 | 采集 .NET Framework 与 .NET 7/8/9/10 安装情况 → `system\dotnet.txt` |
| 10 | 自身文件复制 | — | `%LOCALAPPDATA%\Lertaro\logs\**` 与 `%ProgramData%\Lertaro\logs\service.log` → `logs\` |
| 11 | `wevtutil.exe` | `qe Application/System /rd:true /f:RenderedXml /e:Events /uni:true /q:<XPath>` | 最近 24 小时、正文提到 Lertaro 的错误事件 → `crash\events\*.xml` |
| 12 | 自身文件复制 | — | WER 报告与四个 `CrashDumps` 目录里已有的 Lertaro 转储 → `crash\wer\`、`crash\dumps\` |
| 13 | `.dump_tools\procdump.exe` | `-accepteula <-mm\|-mac\|-ma> -o <PID> PROCESSNAME_PID.dmp` | **仅当目标进程在运行**：按第 2 步的回答选类型；`PROCESSNAME`、`PID` 由 Procdump 替换成 `<进程名>_<进程ID>` |
| 14 | `.dump_tools\7z.exe` | `a -tzip <-mx=0\|-mx=1> ...` | 打包 ZIP；只有落到未压缩完整转储时才用 `-mx=1` |
| 15 | 控制台提问 + `rundll32.exe` | `url.dll,FileProtocolHandler <上传站点>` | **仅当用户在提问里回答“是”**：把上传页面交给默认浏览器打开；程序自己不联网、不上传 |

第 3–12 与第 14、15 步每次运行都做；第 2、13 步只在目标进程正在运行时出现（第 15 步的提问每次都会问，只有回答“是”才执行 `rundll32.exe`）。也就是说不管 Lertaro 有没有在跑，日志和系统里已有的崩溃转储都会一并收走。

内嵌的 fastfetch / procdump / 7-Zip 在运行时解压到 `dump_file\.dump_tools\`（与采集目录同级，不会被写进 ZIP），运行结束后**不会自动删除**，可自行清理。

单个步骤失败只记录到 `run.log`，不影响其余采集与打包。

## 开头的问题：是不是内存问题

只有目标进程正在运行时才会问，答案是“否”时用迷你转储，包体积能小一个数量级（下面是本机实测）：

| 回答 | 实际命令 | 转储体积 | 打包后 | 能查出什么 |
| --- | --- | --- | --- | --- |
| 否（默认） | `-mm` | 23.7 MB | 23.8 MB（`-mx=0`） | 全部线程栈与崩溃点、模块与版本、句柄、地址空间：适合闪退、无响应、启动失败、加载了哪个模块 |
| 是 | 先 `-mac`，被系统拒绝则 `-ma` | 完整 649 MB | **205 MB（`-mx=1`，约 10 s）** | 上面全部 + 堆内存：`!dumpheap`/`!do`/`!gcroot`，也就是越用越吃内存、对象为什么是 null、缓存/列表内容这类问题 |

- `-mac`（内容等同 `-ma`、内存段自带压缩）在本机被系统直接拒绝：`Error writing dump file: 0x80070057`，对 `lertaro.app.exe` 和普通 `cmd.exe` 都一样，加 `-cp`、`-r` 也无效，所以程序自动退回 `-ma` 并在 `run.log` 记 `compressed dump refused`。只有报“写盘失败”才重试；`Error opening`（权限不够、进程不存在）不会重复尝试，服务进程的拒绝仍然只记一行。
- 落到 `-ma` 时 ZIP 才用 `-mx=1`：实测 10 s 把 649 MB 压到 205 MB。再往上升档没有意义——同一份转储 `-mx=3` 只比 `-mx=1` 小 6 KB，时间一样。其他情况仍用 `-mx=0`，因为迷你转储本身已经很小，自带压缩的转储也压不出多少。
- 两个问题都是读一行输入：直接回车、或者没有输入可读（stdin 结束，例如无人值守地重定向）都按默认“否”处理，不会卡在等待上。

## 结束后的问题：要不要上传

打包完成后空一行，再问是否打开上传站点 <https://app.dashbeam.net/>：

- 回答“是”才用 `rundll32.exe url.dll,FileProtocolHandler` 把页面交给默认浏览器；程序自身不联网、不上传，也不会替你发送任何文件。若系统返回调用失败，会打印站点的完整地址请你手动打开，不会静默跳过。
- 把诊断包拖进页面上传，再把页面给出的**一次性传输票据**发给开发者即可。该方式不限速、无需登录，但**开发者接收完成之前不要关闭这个浏览器窗口**。
- 请不要用百度网盘等限速网盘传输诊断包：下载会被限速，开发者往往拿不到完整文件。

## 控制台输出

每一步打印一行状态，并在运行期间每秒刷新自己的耗时，避免长时间看起来像卡住：

```log
正在检查 Lertaro 进程（0 min 1 s）
本次问题是否与内存有关（越用越占内存、内存泄漏、内存暴涨）？（是/否，默认否）
正在采集系统信息（0 min 2 s）
正在读取目录权限（0 min 3 s）
正在检查账户权限（0 min 1 s）
正在检查 .NET 环境（0 min 1 s）
正在复制应用日志（0 min 1 s）
正在查询事件日志（0 min 4 s）
正在收集崩溃报告（0 min 1 s）
正在收集崩溃转储（0 min 0 s）
正在转储 lertaro.app.exe (PID 32048)（0 min 2 s）      ← 仅目标进程在运行时
正在压缩诊断文件（0 min 11 s）                        ← 完整转储配 `-mx=1`；迷你转储约 1 s
诊断包已生成：D:\...\dump_file\2026-10-14_21-34-59-267-7884.zip   ← 打印的是完整绝对路径

是否需要现在打开上传站点，把这个文件交给开发者？（是/否，默认否）
请不要使用百度网盘等限速网盘传输诊断包：下载会被限速，开发者往往拿不到完整文件。
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

目录不存在时写一行 `(directory does not exist)`，`icacls` 调用失败时把错误写在对应目录下，都不中断；一个候选目录都没有时写 `(no directory to inspect)`。

### 3. 账户与权限（`system\account.txt`）

- `whoami /user`：当前账户名与其 SID
- `whoami /groups`：进程令牌中的组、属性与完整性级别（`Mandatory Label`）。**注意**：用 Microsoft 账户登录时，列表里会有形如 `MicrosoftAccount\你的邮箱` 的声明，邮箱地址因此出现在报告中
- `whoami /priv`：当前进程的权限（Privilege）列表，例如 `SeDebugPrivilege` 决定 Procdump 能否打开其他进程
- UAC 策略（注册表 `HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System`，只读）：`EnableLUA`、`ConsentPromptBehaviorAdmin`、`ConsentPromptBehaviorUser`、`PromptOnSecureDesktop`、`FilterAdministratorToken`、`LocalAccountTokenFilterPolicy`；值不存在记为 `(absent)`
- `[summary]`：由令牌推导的三行结论——`local administrator`（账户是否属于本地管理员组，即能否请求提权）、`integrity`（Low/Medium/High/System 及对应 SID）、`already elevated`（当前进程是否已提权）

判定依据是本地管理员组的固定 SID `S-1-5-32-544` 是否出现在令牌中，不依赖语言版本；本程序正常以管理员身份运行，所以通常是 `High` + `already elevated: yes`。某段 `whoami` 输出取不到时对应小节写 `(unavailable)`，UAC 值不存在时写 `(absent)`。

### 4. .NET 运行环境（`system\dotnet.txt`）

- .NET Framework：读 `HKLM\SOFTWARE\Microsoft\NET Framework Setup\NDP\v4\Full` 与 `v4\Client` 的 `Install`/`Release`/`Version`，按微软公布的 Release 阈值推断已装版本，并逐项给出 `4`、`4.5`、`4.8` 是否存在（阈值推断只说明“至少是这个版本”）
- .NET 7/8/9/10：扫描 `%ProgramFiles%\dotnet`（x64）与 `%ProgramFiles(x86)%\dotnet`（x86）下 `shared\Microsoft.NETCore.App`（运行时）、`shared\Microsoft.WindowsDesktop.App`（WPF 依赖的桌面运行时）、`sdk` 三类版本目录，列出实际版本号并按主版本给出 `7`/`8`/`9`/`10` 清单
- 这一项只记录注册表值与目录名，不读取这些安装目录内的任何文件内容
- 占位写法：注册表键或 `dotnet` 目录不存在记 `(absent)`，某类组件没有版本目录记 `(none)`，主版本清单里没有对应版本记 `not installed`

### 5. 日志文件（只读复制，不移动、不删除、不修改源文件）

不论 Lertaro 此刻有没有在运行，下面这些都会收走：

- `%LOCALAPPDATA%\Lertaro\logs\**` → `logs\user\`（递归复制全部用户日志；日志内容可能包含访问过的文件名与路径、搜索关键字等）
- `%ProgramData%\Lertaro\logs\service.log` → `logs\service\`

### 6. 崩溃相关

同样无条件收集：

- Windows 事件查看器 Application / System 通道最近 24 小时的错误事件，且事件正文中提到 Lertaro 的才写入 `crash\events\`
- Windows 错误报告（WER）`%LOCALAPPDATA%\Microsoft\Windows\WER` 与 `%ProgramData%\Microsoft\Windows\WER` 下的 `ReportArchive`、`ReportQueue` 中，报告对象为 Lertaro 程序的文件夹 → `crash\wer\<user|machine>\<队列名>\`（内含计算机名、用户名、模块路径等）
- 用户、系统账户（`%SystemRoot%\System32\config\systemprofile`）、LocalService、NetworkService 四个 `AppData\Local\CrashDumps` 目录中，文件名以 `lertaro.app.exe.` 或 `lertaro.service.exe.` 开头且以 `.dmp` 结尾的文件 → `crash\dumps\<user|system|local-service|network-service>\`
- 目标进程在运行时的**内存转储**，类型由开头“是否与内存有关”的回答决定（否 → `-mm` 迷你，不含堆；是 → `-mac`，被拒绝时退回 `-ma`，含全部内存）。完整转储是进程当时内存的原样快照，可能包含剪贴板内容、正在编辑的文本、已登录令牌等敏感数据，请务必确认接收方可信后再分享。

### 7. 其他

`run.log` 记录本次运行中失败的步骤；进程列表本身只用来决定要不要提问、要不要额外抓内存转储，不会整份写入 ZIP。

## 不做什么

- 不读取浏览器数据、凭据管理器与其他应用程序的文件
- 注册表只查询、不写入：枚举卸载项列表以定位安装目录（结果里只保留 Lertaro 相关的路径），另外只读固定的 UAC 策略键与 .NET Framework 安装键
- 不联网，不做任何自动上报；只有在结束提问里回答“是”时，才用 `rundll32.exe` 把上传页面交给默认浏览器打开，程序不发送任何数据
- 不修改、移动或删除任何被采集的文件；写入的只有 `dump_file\` 与 `dump_file\.dump_tools\`
- 不安装服务、不改注册表（只查询）、不改任何目录的权限（`icacls` 仅读取）

## 产物结构

```tree
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

每次无参数运行都会新建以“时间戳-进程ID”命名的会话目录，旧会话不会被覆盖，也不会被后一次的 ZIP 收录。打包先写成同名的 `.pending.zip`，7-Zip 无警告结束后才改名成 `.zip`，所以中途被打断不会留下一个看起来完整的半成品包。

## 第三方组件

程序内嵌以下工具，运行时解压使用，各自许可见 `res\` 下的许可文件：

- 7-Zip `7z.exe` / `7z.dll` —— LGPL 等，见 `res\7z-license.txt`
- fastfetch —— MIT，见 `res\fastfetch-license.txt`
- ProcDump —— Microsoft Sysinternals 软件许可条款

本项目自身采用 MIT 许可，见 `LICENSE`。
