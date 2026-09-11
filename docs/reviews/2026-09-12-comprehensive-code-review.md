# 全项目代码审查（2026-09-12）

## 1. 结论与审查边界

**结论：当前版本可构建、现有测试通过，但还不能视为完成可靠性验收。首先修历史数据守恒，再处理状态真实性、采集调度、进程操作与隐私防线。**

- 审查基线：`main` 的 `fbe2f5b` **加本次审查开始前已有的未提交修改**，包括容量计算、对比度、SIGTERM 及其测试；不是仅审查 GitHub 版本。
- 已逐文件检查 React/TypeScript 源码、Rust 所有业务模块、Python 探测脚本、构建/发布脚本、配置、测试，以及需求/模型/验收文档。
- 此轮没有修代码、修改设置、提交、推送、重建 `.app`/DMG、修改登录项或触发休眠。Rust 测试只终止测试自身创建的 `/bin/sleep` 子进程；未向真实应用发信号。
- 复现使用内存 SQLite、隔离测试设置文件、模拟命令输出；未打开真实监控数据库。临时复现程序直接引用当前 Rust 源文件，不以另写一份业务逻辑替代验证。
- 未开展新一轮桌面 UI/E2E、持续资源挂测、真实睡眠唤醒、全盘压力或联网漏洞库扫描。静态审查不能替代这些证据。
- 文中链接/行号以当前工作区为准。实施计划见 [分步整改计划](../plans/2026-09-12-code-quality-remediation-plan.md)。

### 优先级与证据

- **P0**：应立即安排止损，继续运行可能丢失保留期内的数据。
- **P1**：下次正式验收前修复的数据正确性、隐私或重要可靠性问题。
- **P2**：后续质量、可用性和性能改进；需要专项验证的口径不直接判定为已发生错误。
- **复现**：实际执行当前实现或受控输入已确认；**静态确认**：可直接从控制流/配置确认；**待专项验证**：确有证据缺口，不宣称已复现。

## 2. 本轮验证结果

| 检查 | 结果 | 能证明什么 |
| --- | --- | --- |
| Rust `cargo test --locked --offline` | 30 通过 | 现有用例通过，不代表边界覆盖完整 |
| 前端 `tsc --noEmit` | 通过 | 类型检查通过；没有重建前端产物 |
| Node 容量计算测试 | 4 通过 | 现有容量派生用例通过 |
| Python AST / Bash 语法 | 通过 | 语法有效；未运行整套硬件探测 |
| `cargo clippy --all-targets -- -D warnings` | **失败** | 7 项诊断，见 A22 |
| `cargo fmt --all -- --check` | **失败** | 59 个格式差异段；没有执行格式化 |
| 历史聚合补充复现 | **发现丢失** | A01 的两层聚合均不守恒 |
| 历史查询补充复现 | **发现漏读** | A02：数据存在但返回空数组 |
| 非法设置文件补充复现 | **被载入** | A13：加载路径没有校验 |
| 探测脚本缺字段模拟 | **错误标 verified** | A25：状态与实际值不一致 |
| 已到达 `main` 的 Git 对象模式扫描 | 133 个内容/元数据对象；所列模式未命中 | 未发现用户主目录、私钥头、常见 token、内网地址、旧私有身份；不是全格式安全保证 |
| 未来 Git 作者/提交者身份 | **不是已批准的匿名身份** | A15：历史清理没有防止再次泄露 |

### 补充复现摘要

**A01：两次聚合跨过未闭合桶。** 调用当前 `HistoryDb::aggregate_and_prune_at`：

1. 插入 `timestamp=1000..1009` 的 10 条样本。以 `now=4605` 聚合：总表示样本数仍为 10；以 `now=4665` 再聚合：只剩 **5**。
2. 插入 `timestamp=6000..6059` 的 60 条样本。以 `now=92425` 聚合：总数 60；以 `now=92485` 再聚合：只剩 **30**。
3. 两次调用都成功提交事务。原因不是事务失效，而是第二次 `INSERT OR REPLACE` 用剩余半桶替换了之前已经聚合、已删除源行的半桶。

**A02：按墙钟选层，而不是按实际存在的数据查询。**

- 插入一条 `timestamp=1000` 原始样本，不做聚合；以 `now=4601` 查询 `[900,4601)`：库内原始行数为 1，返回点数为 **0**。
- 插入 `timestamp=1005`，以 `now=4610` 聚合后查询 `[1005,4610)`：样本在范围内，但桶起点是 1000，返回 **0**。需要明确部分桶语义，不能静默丢掉边界覆盖。

**A13：设置校验只在保存时执行。** 隔离文件写入 `version=999`、前后台间隔均为 0、无效语言。`load()` 返回这些值，而同一个对象调用 `validate()` 会报错。

**A25：缺值被当作已验证。** 用完全模拟的 `PerformanceStatistics={"unrelated"=1}`，GPU 利用率与内存所需字段均缺失，但探测结果仍为 `verified`；CPU 拓扑、物理内存总量为空时也可被标为 `verified`。

## 3. 值得保留的实现

- 历史 SQL 参数化、事务内聚合后删除的方向正确；应修复桶覆盖规则，而非推倒重写数据库。
- 历史查询已有指标白名单、7 天范围和 2,000 点返回上限。
- 容量派生已补齐 `ceiling - free`，保留真实零，对缺值/越界/重复容器返回未知。
- React 以文本渲染进程名，没有发现 `dangerouslySetInnerHTML` 或任意 shell 命令 IPC。
- 终止进程只允许 SIGTERM，拒绝 PID 0/1、自身、不同启动时间和非当前用户；明确提示“已发送不代表已退出”。这些保护应保留。
- 语言选择器、采集状态标签、侧栏选中态的最新颜色修正有效，不能因为其他组件仍有问题而将已完成部分全部否定。
- 构建脚本有 `set -euo pipefail`、ad-hoc 严格签名与 DMG 校验；没有把本地签名冒称公证。

## 4. 问题与建议

### A01 · P0 · 聚合跨桶覆盖，保留期内统计丢失【复现】

位置：[history.rs:119–179](../../monitor-app/src-tauri/src/history.rs:119)。

原始层按 `timestamp < now-3600` 选行，并不保证整个 10 秒桶闭合；10 秒转 60 秒同样按非对齐边界选择。两层都使用 `INSERT OR REPLACE`，下一轮覆盖旧统计后，已删源样本不可恢复。现有“幂等”测试只在**相同 now** 连续调用，且时间固定值与桶边界对齐，未覆盖生产中的滚动边界。

建议：先安排源样本删除止损；再按完整闭合桶聚合，处理迟到样本/时钟回拨，验证 count、sum、min/max、first/last 的守恒。不要简单把 REPLACE 换成加法 UPSERT：未证明源集合不重叠前会引入重复计数。已丢失部分不得补造。

### A02 · P1 · 分层查询存在盲区，保留期边缘提前缺失【复现＋静态确认】

位置：[history.rs:184–192](../../monitor-app/src-tauri/src/history.rs:184)、[history.rs:207–263](../../monitor-app/src-tauri/src/history.rs:207)。

查询只按当前时间推断某区间必定在 raw/10s/60s 哪一层，未考虑每 60 秒运行一次的聚合延迟、聚合失败与重启积压。桶查询仅按 `bucket_start`，可能漏掉实际覆盖查询起点的桶。保留清理按桶起点删整个桶，也可能提前丢掉桶内仍处于 7 天窗口的样本覆盖。

建议：引入已提交聚合水位或跨层去重查询；对查询区间与桶覆盖相交明确语义，返回粒度及覆盖区间。7 天边缘允许保守多保留一个粗桶，不得提前删除窗口内覆盖。

### A03 · P1 · 来源失败被吞掉，旧读数仍标“实时采集”【静态确认】

位置：[sampler.rs:135–149](../../monitor-app/src-tauri/src/sampler.rs:135)、[sampler.rs:372–387](../../monitor-app/src-tauri/src/sampler.rs:372)、[lib.rs:83–88](../../monitor-app/src-tauri/src/lib.rs:83)、[App.tsx:150–198](../../monitor-app/src/App.tsx:150)。

GPU/内存/吞吐任一错误可丢弃整个快照；调度器用 `.ok()` 吞错，缓存保留旧值。IPC 仍成功返回旧缓存，前端因此清除错误。虽然 DTO 有 `observed_at`，前端没有过期判断。首次 GPU 失败还会挡住 CPU、内存、存储乃至独立的进程页面。

建议：各来源独立结果与状态；公开 last-success、last-attempt、age、错误码。全局状态根据采集健康而非 IPC 是否成功判断。进程页不应依赖 GPU 快照就绪。

### A04 · P1 · 实际采样周期是“等待＋全部采集耗时”，阻塞来源无超时【静态确认】

位置：[sampler.rs:177–314](../../monitor-app/src-tauri/src/sampler.rs:177)、[sampler.rs:358–387](../../monitor-app/src-tauri/src/sampler.rs:358)、[lib.rs:33–39](../../monitor-app/src-tauri/src/lib.rs:33)。

每轮先睡配置间隔，再串行执行系统命令；仅 `iostat -c 2 -w 1` 就引入约一秒观测等待，不能把配置 1s/3s 当实际 1s/3s。每轮还重新查询 GPU 静态名称、全量 ioreg、全部磁盘拓扑。同步 `Command::output()` 在 async 任务中执行，没有截止时间或退出时子进程回收策略。

建议：稳定时间基准调度并跳过 missed ticks，避免追赶风暴；分离静态/低频拓扑与动态采样；使用受控阻塞 worker 或异步子进程，限制并发、输出大小、超时并 reap 子进程。以实际耗时 P50/P95 和间隔分布验收，不能仅改 sleep API。

### A05 · P1 · 历史写入非快照原子，存储错误不可见，DB 故障阻止实时查看【静态确认】

位置：[sampler.rs:318–333](../../monitor-app/src-tauri/src/sampler.rs:318)、[history.rs:97–102](../../monitor-app/src-tauri/src/history.rs:97)、[history.rs:318–339](../../monitor-app/src-tauri/src/history.rs:318)、[lib.rs:141–144](../../monitor-app/src-tauri/src/lib.rs:141)。

每项指标单独 INSERT、逐次锁库；写盘失败被 `let _ =` 忽略，可能留下半个快照。聚合失败也不报告；查询用 `rows.flatten()` 忽略单行解码错误。初始化 DB 使用 `expect`，损坏/不可写会让整个应用无法启动，违背实时查看可降级运行的验收要求。

建议：批量事务写入一轮快照；历史故障独立于实时采集，错误可见且日志脱敏；明确 busy/retry 上界和队列容量。迁移检查 schema version；禁止“出错即重建空库”。

### A06 · P1 · 进程 I/O 无权限/瞬时失败变成零，恢复后可能形成假峰值【静态确认】

位置：[processes.rs:78–110](../../monitor-app/src-tauri/src/processes.rs:78)、[processes.rs:163–169](../../monitor-app/src-tauri/src/processes.rs:163)。

`disk_io_bytes(...).unwrap_or((0,0))` 把不可读计数加入基线：持续失败会显示 0 B/s；失败后恢复时，累计全生命周期字节可能被除以一个短间隔，形成假峰值。进程数量字段也不等于真正可读 I/O 的进程数。

建议：累计值、速率和可读状态分开；失败使该来源基线失效，恢复第一帧预热。增加成功→失败→恢复、连续失败、计数回退的纯逻辑测试。首次基线测试不要允许 `Some(0)` 混入应为 None 的断言。

### A07 · P1 · 测试数据目录仅隔离数据库，设置仍读写真实目录【静态确认】

位置：[history.rs:304–315](../../monitor-app/src-tauri/src/history.rs:304)、[lib.rs:140–144](../../monitor-app/src-tauri/src/lib.rs:140)、[settings.rs:82–99](../../monitor-app/src-tauri/src/settings.rs:82)。

`MONITOR_DATA_DIR` 只在 history 模块解析；settings 仍使用默认 app_data_dir，而且默认目录会先被创建。因此此前“隔离实例完全不碰真实数据”的表述过强，切换测试设置可能改动真实配置。

建议：只解析一次统一 DataPaths，数据库、设置、实例锁、日志都使用相同隔离根。测试以哨兵文件证明真实目录未变；7 天挂测用持久隔离目录而非容易清理的临时目录。

### A08 · P1 · 无同数据目录单实例约束，多实例会污染历史/设置【静态确认】

位置：[lib.rs:129–202](../../monitor-app/src-tauri/src/lib.rs:129)、[history.rs:97–100](../../monitor-app/src-tauri/src/history.rs:97)、[settings.rs:69–76](../../monitor-app/src-tauri/src/settings.rs:69)。

每次启动都开调度器并写同一默认数据库；没有跨进程锁、快照唯一约束或采集 session 区分。SQLite 锁保证事务串行，不保证两次测量不是重复样本。两个设置存储器共用固定 `.json.tmp` 名也可互相干扰。

建议：以数据目录为粒度加单实例锁；同目录第二实例仅唤起已有窗口，不启动采样。不同隔离目录允许并行。不要通过直接终止已有用户实例解决冲突。

### A09 · P1 · APFS 多物理存储被归给第一块盘，部分容量伪装全盘用量【静态确认】

位置：[storage.rs:44–56](../../monitor-app/src-tauri/src/storage.rs:44)、[storage.rs:171–178](../../monitor-app/src-tauri/src/storage.rs:171)、[App.tsx:206–243](../../monitor-app/src/App.tsx:206)。

实现取 `PhysicalStores.first()`，没有保留注释所说的数量；跨盘容器会被完整归给第一块盘。仅一部分分区为 APFS 时，界面虽列明占用率分母，但“已使用”仍容易被理解为整块物理盘已用。无 APFS/拓扑失败与真正没有设备也不能区分。

建议：保留全部 backing stores，容器作为共享资源池；无法拆分的多盘共享容量不要归给单盘。显示“APFS 已用”，记录覆盖完整性；非 APFS 明确未接入，而非猜零。补 Roles 数组兼容解析（当前仅读 `Role` 字符串）。

### A10 · P1 · 历史以 diskN 为设备身份，热插拔/重启后可能串盘【静态确认】

位置：[sampler.rs:326–332](../../monitor-app/src-tauri/src/sampler.rs:326)、[storage.rs:103–110](../../monitor-app/src-tauri/src/storage.rs:103)、[App.tsx:664–690](../../monitor-app/src/App.tsx:664)。

`disk0`/`disk4` 是本次枚举标识，不是永久硬件身份。同一编号被其他设备复用时会接到旧历史；两个同型号设备也不能用名称去补救。

建议：引入本地匿名设备 ID 与来源代次。不能为追求跨重启关联而擅自记录序列号/UUID；在现有隐私边界内无法可靠关联时，创建新代次、保留旧历史但明确不合并。

### A11 · P1 · 结束进程可保持已不在当前列表的旧选择【静态确认】

位置：[App.tsx:765–817](../../monitor-app/src/App.tsx:765)、[App.tsx:850–885](../../monitor-app/src/App.tsx:850)。

搜索、排序、轮询替换 page 后不清理 selected；用户可以在新的列表里继续操作已隐藏的旧目标。确认面板打开后，即使后续 loadError 出现，确认按钮只检查 ending，仍可提交。后端身份校验能阻止部分误操作，但不能消除 UI 目标错觉。

建议：确认时冻结不可变目标，并显示身份/可见性状态；选择离开列表、列表过期或失败时取消确认或要求重新选择。使用发送锁和请求 ID 防重复提交；提供 Esc、取消后焦点恢复，并在轮询中确认是否退出，不把成功发信号当成已退出。

### A12 · P1 · 登录项 AppleScript 的路径转义方式错误【静态确认】

位置：[loginitem.rs:9–20](../../monitor-app/src-tauri/src/loginitem.rs:9)、[loginitem.rs:47–55](../../monitor-app/src-tauri/src/loginitem.rs:47)、[lib.rs:103–120](../../monitor-app/src-tauri/src/lib.rs:103)。

这里用的是 shell 单引号转义，却把路径插入 AppleScript 双引号字符串。引号/反斜线/换行路径可破坏脚本语法，并存在脚本注入面；路径取自当前可执行文件，并非声称远程任意输入已可利用。非 bundle 开发运行时按祖先层数猜 `.app` 路径也不可靠。

查询失败用 `unwrap_or(enable)` 代替实际状态，保存失败被忽略，可能向用户虚报注册结果；重复 add 未检查幂等性。

建议：固定脚本通过 argv 传路径，验证真实 bundle；注册、核验、保存分步返回结果，失败不猜成功。真实登录项写操作仍需用户专项授权，优先采用脚本生成/适配器模拟测试。

### A13 · P1 · 设置加载绕过校验，版本不生效；UI 保存存在竞态【复现＋静态确认】

位置：[settings.rs:42–66](../../monitor-app/src-tauri/src/settings.rs:42)、[settings.rs:69–94](../../monitor-app/src-tauri/src/settings.rs:69)、[App.tsx:914–977](../../monitor-app/src/App.tsx:914)。

加载不调用 validate；version/language 不验证，损坏与访问错误静默回默认。UI 每次输入立刻异步保存整个对象，可能发送 0/中间值、旧状态覆盖新值；失败时 UI 没有回滚。首次加载失败仍落到“正在加载设置”，错误信息不可见。语言同时有 localStorage 和 Rust 设置两个来源。

建议：加载时版本迁移及校验，保留坏文件并可见提示；内存设置缓存，串行/修订号保存，唯一临时文件与必要的 fsync。前端草稿与已保存状态分离，明确提交或防抖队列。语言选一个唯一持久来源。

### A14 · P1 · 探测报告有路径收集入口，默认输出目录不受忽略规则保护【静态确认】

位置：[probe-all.py:18–24](../../scripts/probes/probe-all.py:18)、[probe-all.py:650–675](../../scripts/probes/probe-all.py:650)、[run-all.sh:8–13](../../scripts/probes/run-all.sh:8)、[.gitignore:6–10](../../.gitignore:6)。

报告可直接写入 `MountPoint`，错误摘要也缺统一脱敏；timeout 异常可能携带命令输出。输出到 `docs/probes/*.json`，但忽略规则仅覆盖 `docs/probes/reports/`。已跟踪报告包含 20 个进程条目，属于可能暴露使用习惯的本机诊断数据。

**本轮现存 7 份报告的非空 mount_point 数为 0，未检出所列敏感模式，不能写成已发生主目录/私钥泄露。** 但生成路径缺少结构性防护。

建议：输出使用字段白名单和错误码，默认写被忽略的本地目录；公开仓库只放合成、明确标识的测试夹具。清理已跟踪真实报告或重写已推送历史涉及共享内容变更，应另获授权；忽略规则不会抹除历史。

### A15 · P1 · 已匿名历史之后的新提交仍会使用非匿名身份【复现】

证据：`main` 历史已是批准的匿名身份；仓库本地 user.name/user.email 均未设置，`git var GIT_AUTHOR_IDENT` / `GIT_COMMITTER_IDENT` 的当前解析结果均不是批准的 noreply 身份。本报告不记录具体私有值。

建议：在后续**任何提交之前**配置仓库级匿名身份，检查作者与提交者；pre-push 扫描待推送提交和 blob，而非只 grep 当前目录。不要修改全局 Git 设置或再次擅自重写远端历史。

### A16 · P2 · SIGTERM 身份检查仍有精度与竞态边界【已声明的限制＋测试缺口】

位置：[termination.rs:8–27](../../monitor-app/src-tauri/src/termination.rs:8)、[processes.rs:78–80](../../monitor-app/src-tauri/src/processes.rs:78)。

当前启动标记使用 sysinfo 的秒级启动时间；检查与 kill 非原子，源码已诚实说明。秒内 PID 复用或 exec 后名称变化属于额外边界。已具备用户 UID 和自保护，不能把此处夸大为已复现的越权漏洞。

建议：用可验证的高精度启动身份/本地代次、失败关闭；评估平台原子定向终止能力，若平台做不到就保留风险声明。补 gone、permission、身份缺失、超时、忽略 SIGTERM 等受控测试。测试子进程用 RAII 回收，防前置断言 panic 遗留进程。

### A17 · P1 · 预热、长暂停后基线重建与最小化节奏没有完整实现【静态确认】

位置：[sampler.rs:75–77](../../monitor-app/src-tauri/src/sampler.rs:75)、[sampler.rs:111–125](../../monitor-app/src-tauri/src/sampler.rs:111)、[lib.rs:206–224](../../monitor-app/src-tauri/src/lib.rs:206)。

`last_tick` 只赋值未读取；注释称调用方视为预热，但实际 DTO/写历史没有预热状态。长暂停/唤醒没有显式差分重置。窗口只处理关闭/焦点，最小化通常仍是 visible，不能据此保证进入后台节奏。

建议：独立生命周期状态机，明确启动/恢复预热、计数回退和时钟跳变；窗口恢复主动设置状态。只用虚拟时钟、暂停采集器和事件注入测试，**不得执行系统休眠/唤醒**。旧“睡眠验收通过”的证据仅能证明当时恢复写样，不能证明所有基线正确。

### A18 · P2 · 7 天历史没有 UI 入口，图表窗口/缺口与聚合语义不完整【静态确认】

位置：[App.tsx:343–429](../../monitor-app/src/App.tsx:343)、[App.tsx:443–466](../../monitor-app/src/App.tsx:443)、[App.tsx:475–621](../../monitor-app/src/App.tsx:475)、[history.rs:259–263](../../monitor-app/src-tauri/src/history.rs:259)。

全部页面固定请求 3600 秒；虽后端接受 7 天，用户无法切换。图表用现有样本 min/max 铺满宽度，不保留请求窗口的首尾空白，没有可读时间轴。gap 固定 5/10 秒，与可配置 30 秒后台间隔、10/60 秒桶及抽样 stride 不匹配，可能把正常采样画成离散点。均匀 stride 会跳过末点/短峰；平均值目前按样本而非实际覆盖时长加权。

建议：响应包含请求起止、粒度、覆盖/缺口信息和聚合方法；增加 1h/24h/7d 控件。视图按真实请求范围画图，保留边界与峰值，不用粗粒度点间隔推断睡眠。先确定时间加权还是样本加权契约，不静默改变历史口径。

### A19 · P2 · 多处轮询无重入/乱序保护；切盘会用新标题展示旧图【静态确认】

位置：[App.tsx:440–460](../../monitor-app/src/App.tsx:440)、[App.tsx:475–491](../../monitor-app/src/App.tsx:475)、[App.tsx:666–690](../../monitor-app/src/App.tsx:666)、[App.tsx:787–817](../../monitor-app/src/App.tsx:787)。

除主快照外，大多使用固定 setInterval 直接发请求，无 busy/request-version；同一 effect 内返回乱序仍可覆盖新值。切盘只取消旧 effect 写入，没有清空已有 history，若新查询失败会一直展示旧盘曲线却标新设备。磁盘拔出时 selected 不会自动校正。隐藏窗口也没主动停前端无用历史轮询。

建议：提取 latest-only、可取消、single-flight 的查询 hook；按对象/窗口缓存；切换目标先显示 loading，不跨设备复用数据；区分 empty/error/stale。

### A20 · P2 · 进程列表“全量分页”只存在后端，用户只能看到前 50 项【静态确认】

位置：[lib.rs:41–65](../../monitor-app/src-tauri/src/lib.rs:41)、[App.tsx:793–817](../../monitor-app/src/App.tsx:793)。

前端固定 offset=0、limit=50，没有翻页；total_readable 是筛选前总量，没有 matched_total。每次搜索键入都会触发全进程扫描并改变差分窗口；CPU 第一帧与系统 CPU 百分比口径也没有说明。

建议：后端独立适度频率采进程快照，IPC 只筛选排序分页；增加过滤后总数、稳定排序 tie-breaker、翻页和搜索防抖；切页清理选择，明确进程 CPU 是否允许超过 100%。

### A21 · P2 · 对比度修复尚未覆盖排序按钮及全部可访问性【静态计算＋待 UI 验证】

位置：[styles.css:1–4](../../monitor-app/src/styles.css:1)、[styles.css:6–24](../../monitor-app/src/styles.css:6)、[App.tsx:857–885](../../monitor-app/src/App.tsx:857)。

`.sort-buttons button.active` 仍白字配浅色渐变：浅色起点对比度约 **3.65:1**；深色两端约 **2.54:1/2.98:1**，未达到普通字号 4.5:1。新侧栏修复没有覆盖它。CSS 长行叠加覆盖难维护；多数字段缺 `<label htmlFor>`，确认区没有 Esc/焦点恢复逻辑，温度迷你图缩放后视觉线宽/单点大小及默认 800×600 布局仍需 WKWebView 实测。

建议：集中语义令牌与交互态矩阵，格式化后分组件 CSS；统一按钮、表单、确认组件。覆盖 800×600、窄屏、长名称、中英文、明暗主题、键盘及减少动态效果；不要只测孤立列表而遗漏整个侧栏布局。

### A22 · P2 · 测试与工程质量门禁不足【复现＋静态确认】

位置：[history_test.rs:27–98](../../monitor-app/src-tauri/src/history_test.rs:27)、[package.json](../../monitor-app/package.json)、[storage-usage.test.mjs](../../monitor-app/tests/storage-usage.test.mjs)、[Cargo.toml](../../monitor-app/src-tauri/Cargo.toml)。

没有 CI 和 npm test 入口；Node 原生 TypeScript 测试依赖运行时能力，却未声明 Node 下限。历史测试偏整齐/常量数据，前端交互测试只在临时页执行，未沉淀回归测试。Rust fmt 检查失败；Clippy 严格检查有 7 项：div_ceil、可省生命周期、两项 Default 后赋值、返回元组类型复杂、sort_by_key、可合并 match/if。

建议：先新增失败回归，再修；Mac CI 固定锁文件与工具版本，执行格式、lint、类型、纯逻辑与受控集成测试。按职责拆 App.tsx（约千行）、语言字典、hook 和 DTO，避免为了重构先大改所有页面。依赖漏洞/许可证扫描未在本轮联网执行，不得宣称零漏洞。

### A23 · P2 · 发布产物可被失败重建覆盖，缺少可追溯的发布清单【静态确认】

位置：[build-release.sh:17–40](../../scripts/build-release.sh:17)、[tauri.conf.json:3–30](../../monitor-app/src-tauri/tauri.conf.json:3)、[Cargo.toml:3–8](../../monitor-app/src-tauri/Cargo.toml:3)。

DMG 名固定版本/架构，未校验当前实际构建架构；先删旧 DMG，再创建新包，打包失败会失去上一次产物。构建不要求干净工作区、不运行测试、不明确锁定依赖，不输出 commit/工作区差异标记/工具链/测试/散列的关联清单。对 bundle 的签名/镜像 CRC 通过不等于 DMG 内应用功能验证通过。最低 macOS 11 的兼容性尚未实测，许可证/仓库元数据仍为空。

建议：在独立 staging 中构建验证，成功后原子发布并保留上一版；生成 release manifest，验证镜像内 app 架构/签名/资源，产物名反映真实版本和架构。固定已验证 Mac 范围，不开展 Windows；许可证由用户选择，不代为决定。继续 ad-hoc，不自动引入 Developer ID/公证。

### A24 · P2 · 文档状态与证据不一致，旧计划可能重复引入缺陷【静态确认】

位置：[README.md:5–21](../../README.md:5)、[capability-matrix.md:3–37](../capability-matrix.md:3)、[remediation-acceptance.md:3–41](../remediation-acceptance.md:3)、[探测契约末尾](../probes/contract.md)。

- README 宣称旧审查 27 项全部修复；本轮复现说明不能据此关闭所有问题。
- 验收记录写“7 天原始样本”，实际是 1h raw→24h 10s→7d 60s；“零警告”不能代表本轮 Clippy 严格检查通过。
- 能力矩阵一面“尚未实时验证”，一面列“已验证”；还把传感器未读到推为“无风扇”，证据不足，不能保留为能力结论。
- 验收文档的旧审查相对链接失效；身份重写后仍引用不可到达的旧 commit。
- 探测契约末尾残留 EOF、mkdir、cat、touch 等脚本文本；旧总览实现计划包含已被修正的 null 跳过求和逻辑，不宜再直接执行。

建议：报告证据不可篡改；旧结论以追加“被本次审查重新打开/已被取代”校正，统一“计划/实现/自动测试/本机实测/仍待验证”列，并为每项验收关联版本和用例。

### A25 · P2 · 探测状态过度乐观，解析边界与主程序不一致【复现＋静态确认】

位置：[probe-all.py:72–91](../../scripts/probes/probe-all.py:72)、[probe-all.py:202–216](../../scripts/probes/probe-all.py:202)、[probe-all.py:397–447](../../scripts/probes/probe-all.py:397)、[probe-all.py:580–617](../../scripts/probes/probe-all.py:580)、[parse.rs:13–48](../../monitor-app/src-tauri/src/parse.rs:13)。

缺字段仍可 verified；探测吞吐未限定物理设备列表，与应用实现不同。温度未经权限拒绝实测就写 unavailable/high confidence。Rust `parse_ioreg_stat("}{",...)` 会因无效切片 panic；某条目无等号可提前中止解析，通用键匹配也不是严格行首匹配。

建议：状态来自必要字段/范围/来源验证，而非命令退出码；未实现保持 not_implemented，权限拒绝要有实际证据。用固定、脱敏夹具复用解析契约，对 malformed 输入、单位和本地化做模糊/属性测试，任何外部输出不得触发 panic。

### A26 · P2 · 内存/GPU/SMART 口径仍需专项验证【待专项验证，不等于读数全部错误】

位置：[sampler.rs:196–223](../../monitor-app/src-tauri/src/sampler.rs:196)、[sampler.rs:252–266](../../monitor-app/src-tauri/src/sampler.rs:252)、[storage.rs:128–147](../../monitor-app/src-tauri/src/storage.rs:128)。

available 将 free/inactive/speculative/purgeable 相加，尚未证明类别不重叠；不能仅凭公式存在就声称等于系统工具“可用”。现存报告未出现 used+available 大于总量，**不据此断言本机已显示超物理容量**。GPU 匹配全量 ioreg 中第一个 PerformanceStatistics，没有按 GPU 身份匹配。SMART 字段明确不保证稳定，温度无来源单位元数据，通电时间只取 `_0`，高段未验证。

建议：记录来源、单位、统计范围和置信状态；验证内存集合关系与 Apple 口径，GPU 对象定位及缺字段范围，SMART 已知设备夹具/多段解释。不要猜温度单位或通电计数，不新增提权采集；未验证能力保持诚实标注。

## 5. 实施顺序建议

1. **提交隐私与数据保护门禁 → 历史聚合止损/守恒 → 查询覆盖与存储故障。**
2. **统一隔离目录/单实例 → 采集健康状态 → 调度与生命周期 → I/O/拓扑/设备身份。**
3. **进程操作状态机 → 设置/登录项 → 历史 UI/并发查询 → 剩余对比度和可访问性。**
4. **诊断脱敏/状态契约 → CI/发布与文档校正 → 安全的长期验收。**

不要先做大规模视觉重构或盲目升级依赖。完整阶段、依赖、测试与退出标准见 [整改计划](../plans/2026-09-12-code-quality-remediation-plan.md)。
