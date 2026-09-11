# 采集能力矩阵与研究证据

日期：2026-09-11。状态：保留此前静态初查证据，实时采集尚未验证。本次依据 D-012 只调整计划，未新增探测；后续先验证 Mac，Windows 延后。

关联：[需求](requirements.md) · [实施方案](implementation-plan.md) · [验收](validation-plan.md)

## 1. 如何阅读

**执行范围：当前仅规划本机 Mac。** 下表 Windows 列及其官方研究保留为延后参考，不是当前采集/测试任务，也不阻塞 Mac 交付。M0 探测须待用户后续允许进入开发后才启动。

- **基础**：D-010 首版基础目标；遇到无法取得的数据必须报告并调整验收约定，不能直接以不可用占位符宣称完成。
- **优先增强**：验证可读且安全条件满足时优先接入，允许明确降级。
- **后续/可选**：原始目标仍保留，但不自动阻塞基础版本。
- **已验证静态**：本次查询实际返回的清单/摘要，不代表动态指标可采集。
- **来源候选**：官方文档或设计判断支持进一步尝试，尚不代表在用户设备上通过。
- **未验证**不等于不支持。Windows 截图仅是硬件/系统资料，不是采集接口测试。

## 2. 指标矩阵

| 指标 | 优先级 | Mac 优先计划：候选路径 / 已有证据 | Windows 延后参考：候选路径 / 已有证据 |
| --- | --- | --- | --- |
| CPU 名称、核心计数 | 基础清单 | 已验证静态：M4、10 核、4P+6E；sysctl/Mach/sysinfo | 截图确认 HX 370；CIM/系统拓扑/sysinfo 尚未实测 |
| CPU 总/每逻辑处理器占用 | 基础 | iostat -c 差分与 sysinfo 逐核均已实现；Rust 调度器按前台 1s/后台 3s 持续采样，关窗不停采 | 系统性能接口/sysinfo；未采样 |
| 核心分组与映射 | 详情目标 | P/E 数量已知，逐核心身份映射未验证，不能假定索引范围 | 系统拓扑与效率类别待查；不能仅凭型号将 Zen 核心分组当 Apple P/E |
| GPU 名称 | 基础清单 | system_profiler 已识别 M4 | 截图确认 Radeon 890M；DXGI/设备清单待测 |
| GPU 总利用率 | 基础 | ioreg PerformanceStatistics 已验证（20260911T034334Z 报告，Device Utilization 27%）；IOKit IOReport/powermetrics 未验证 | GPU Engine 系统计数器/WDDM 路径；实例映射与最忙引擎口径待测 |
| GPU 内存用量/容量/预算 | 基础但须适配架构 | ioreg PerformanceStatistics 已验证（20260911T034334Z 报告，In use 1.04GB / Alloc 3.85GB）；统一内存，无独立显存分母 | GPU Adapter Memory 等候选；8 GB 截图值不是独立显存芯片证明；专用/共享/预算口径待测 |
| 系统内存总/已用/可用 | 基础 | 总量 16 GiB 已验证静态；memory_pressure + sysctl 动态口径已验证（20260911T034123Z 报告，used 74.06%）；进程内存排行未实现 | 安装 64 GB、系统可使用 55.6 GB 来自截图；GlobalMemoryStatusEx/性能接口/sysinfo 待测 |
| 进程内存排行 | 基础配套 | 已实现：sysinfo 枚举 + 全体可读进程排序/搜索/分页；内存为 RSS。不含 footprint；权限受限进程无法读取时该进程不进入列表 | 进程内存 API/sysinfo；工作集/私有内存及权限范围待测 |
| 进程存储读写排行 | 基础配套 | **已实现（普通权限）**：proc_pid_rusage(RUSAGE_INFO_V4) 累计磁盘读写字节，按 (pid, 启动标记) 差分得速率；系统范围、不按盘归因（D-009）；PID 重用在差分基线中分离 | 存储事件/计数来源待验证；不能把 GetProcessIoCounters 的全部 I/O 当纯存储 |
| 物理盘名称/容量 | 基础清单 | 已识别两块 SSD 及字节容量；设备编号是本次枚举值，不是永久身份 | 截图仅有 2.77 TB 汇总；物理盘数量/型号待测 |
| 卷容量与盘/容器/卷关系 | 基础 | 已实现：物理盘→APFS 容器→卷三层拓扑，按容器归属到物理盘；共享容量按容器统计一次，卷容量为消耗值不重复累计 | 存储 IOCTL、卷盘区映射、容量 API；未探测 |
| 物理盘读写 MB/s | 基础 | iostat -d 按盘拆分已验证（20260911T034529Z 报告，4 盘设备）；需过滤非物理盘（disk10/12/14） | PhysicalDisk/存储性能接口候选；未采样 |
| CPU/GPU 摄氏温度 | 优先增强 | 普通权限不可用（20260911T035248Z 报告）；powermetrics 需 root；SMC 私有 API 需 IOKit | LHM/厂商接口候选；可能涉及驱动；未验证 |
| 磁盘温度/通电时间 | 优先增强 | **已实现（普通权限）**：diskutil `SMARTDeviceSpecificKeysMayVaryNotGuaranteed` 的 TEMPERATURE（Kelvin 转换，0-150°C 范围校验）与 POWER_ON_HOURS_0；温度已进入历史曲线。多段高字节字段（POWER_ON_HOURS_1）未验证 | 存储协议/健康接口、合规候选库；未取得数值 |
| 磁盘健康/寿命磨损 | 优先增强 | 普通权限不可用（20260911T035248Z 报告）；SMART Verified 摘要非健康详情；磨损、寿命百分比需增强权限 | SMART/NVMe 或系统可靠性计数候选；未验证 |
| 风扇转速 | 可选 | 不适用（20260911T035248Z 报告）；Mac mini M4 无风扇；ioreg 未见风扇传感器 | 主板/EC/LHM 候选；未验证 |
| CPU/GPU 频率、功耗、电压 | 后续，易获取项可提前 | 帮助列出 cpu_power/gpu_power；功耗说明为估计；频率/电压与温度均需分别验证 | 系统/厂商/LHM 候选；当前型号与安全配置支持未知 |
| 内存 SPD/时序/电压/温度 | 后续 | 未验证；统一内存不能套用 DIMM 模块模型 | 截图只有 5600 MT/s；SPD/时序/电压/温度并未验证 |
| GPU 引擎细分 | 后续 | GPU 引擎/资源统计接口待研究，不由名称推测 | 系统引擎计数器候选；归属、口径与采样窗口待测 |
| 活动时间/响应时间/队列 | 后续 | 各指标来源/公式逐项验证，不能由 MB/s 推算忙碌程度 | PhysicalDisk/存储事件候选；实例、窗口和物理层归属待测 |

任何派生健康度、频率平均、功耗估计或 GPU 聚合都必须保留方法和来源说明。

## 3. 此前已取得的证据（本次仅保留）

| 编号 | 操作 | 实际结论 | 不能推出的结论 |
| --- | --- | --- | --- |
| E-01 | sw_vers、uname、sysctl 只读查询 | macOS 26.5.2/M4/arm64；CPU 核心计数和 16 GiB 内存 | CPU/GPU 温度、实时利用率或逐核分组映射可读 |
| E-02 | system_profiler GPU 清单，输出白名单过滤 | Apple M4 GPU 可识别 | 已取得全局 GPU 利用率、显存用量或 GPU 核心数 |
| E-03 | diskutil 物理盘清单和逐盘摘要，输出白名单过滤 | APPLE SSD AP0256Z 与外置 ZHITAI TiPlus7100 1TB 可识别，容量和 SMART Verified 摘要可读 | 已读取温度、通电小时、寿命百分比、盘性能或卷布局 |
| E-04 | powermetrics --help | 本机列出 cpu_power、gpu_power、thermal 等采样器；支持 plist 输出，文档描述功耗为估计值 | 采样器在当前权限下可运行、返回每个目标字段或满足 1 秒低开销采样 |
| E-05 | 用户 Windows 系统截图 | Win11 Pro 25H2、HX 370、890M、64 GB 内存等静态资料 | Windows 驱动、计数器、传感器或探测程序已验证 |
| E-06 | 编译器/工具版本查询 | Rust、Node、npm、Swift 可运行；活动 Apple 工具目录为 CLT；未找到 dotnet | 桌面应用已构建、helper 可安装或 Windows 包可生成 |
| E-07 | 官方网页、文档和源码元数据只读核对 | 存在候选框架/API，LHM v0.9.6 有 PawnIO 模块及多目标框架 | 已完成依赖安全审计、许可合规或驱动签名验证 |

未运行实时监控探测，未执行负载测试，未调用 sudo，未注册服务或安装驱动。

## 4. 技术陷阱与验证方法

### GPU 内存

- Microsoft 的 `IDXGIAdapter3::QueryVideoMemoryInfo` 说明的是**当前进程的预算和使用量**，不能拿采集器自身数据当整机 GPU 用量。
- Metal 的 `recommendedMaxWorkingSetSize` 是性能相关的近似工作集建议，不是独立显存总容量。`currentAllocatedSize` 等资源统计也必须核实观察范围，不能只看到“device”就认定为全系统统计。
- 验证时需要用其他进程的 GPU 分配/释放观察全局读数变化；仅在监控程序自身创建资源后能变化，不足以证明是整机统计。
- Apple Silicon 不强制显示不存在的“独立显存占用率”；但若 D-010 目标中的 GPU 内存统计无法取得，必须报告并确认替代口径，不能以系统 RAM 占用率冒充。

### Windows 进程 I/O（延后验证）

- `GetProcessIoCounters` 文档称其覆盖指定进程全部 I/O 操作，不能直接当成纯磁盘读写量。
- 需要区分一般设备/网络 I/O 与存储 I/O；进程侧逻辑读写、缓存和物理盘实际吞吐也不是同一个口径。
- 通过受控的存储读写与纯网络活动分别验证；若候选值在纯网络活动时也增长，应按实际范围命名，不能通过界面文案掩盖。
- ETW 若被采用，必须只启用必要事件、限制缓冲与开销，不记录文件内容或实际读写路径，不在完全退出后保留采集会话。

### 驱动、温度与健康

Windows 的 LHM/PawnIO/驱动条目均延后，仅保留已有研究；Mac 温度与健康是后续 Mac 验证计划的一部分，本轮均不执行。

- LHM 的主许可与随带模块/驱动许可分别检查，不能只看仓库首页的一个许可证。
- 发布说明提到 PawnIO 更新不等于驱动已通过本项目的签名、安全或系统兼容性检查。
- Mac thermal pressure 与摄氏温度分开；SMART pass/fail、厂商磨损值和推算剩余寿命分开。
- 设备名、字节容量或缺少读数不能用来猜测温度、寿命、风扇是否存在或转速是否为 0。

## 5. 官方资料

以下资料于 2026-09-11 读取，网络文档后续可能变化；动态文档版本不能替代最终依赖锁定。

- [Tauri 2 开发前置条件](https://v2.tauri.app/start/prerequisites/)：桌面 macOS 可使用 CLT；Windows 开发涉及 MSVC Build Tools 与 WebView2。
- [Tauri 2 安全边界](https://v2.tauri.app/security/)：系统 WebView、IPC 与 capabilities 约束。
- [sysinfo 0.39.6 文档](https://docs.rs/sysinfo/0.39.6/sysinfo/)：按需刷新、CPU 差分与更新间隔、平台/商店限制；本次读取 latest 页面时其显示为 0.39.6。
- [Apple SMAppService](https://developer.apple.com/documentation/servicemanagement/smappservice)：管理 app bundle 内 helper、登录项、agent/daemon；可用性不等于 helper 安装已验证。
- [Apple Metal currentAllocatedSize](https://developer.apple.com/documentation/metal/mtldevice/currentallocatedsize) 与 [recommendedMaxWorkingSetSize](https://developer.apple.com/documentation/metal/mtldevice/recommendedmaxworkingsetsize)：资源分配与建议工作集接口，不能混为物理显存容量。
- [Microsoft GPU 任务管理器统计说明](https://devblogs.microsoft.com/directx/gpus-in-the-task-manager/)：最忙引擎、WDDM、专用/共享内存的语义。此资料解释方法，不证明当前 890M 接口已实测。
- [Microsoft QueryVideoMemoryInfo](https://learn.microsoft.com/en-us/windows/win32/api/dxgi1_4/nf-dxgi1_4-idxgiadapter3-queryvideomemoryinfo)：进程预算与用量。
- [Microsoft GetProcessIoCounters](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-getprocessiocounters)：指定进程全部 I/O 的统计范围。
- [LibreHardwareMonitor 官方项目](https://github.com/LibreHardwareMonitor/LibreHardwareMonitor)：README 说明部分传感器需要管理员权限，主许可 MPL 2.0，另有第三方条款。
- [LibreHardwareMonitor v0.9.6 发布](https://github.com/LibreHardwareMonitor/LibreHardwareMonitor/releases/tag/v0.9.6)：官方 latest 接口本次返回该版本，发布时间为 2026-02-14；发布说明包含 PawnIO 模块更新。
- [v0.9.6 库工程配置](https://github.com/LibreHardwareMonitor/LibreHardwareMonitor/blob/v0.9.6/LibreHardwareMonitorLib/LibreHardwareMonitorLib.csproj)：包含 net472/netstandard2.0/net8.0/net9.0/net10.0 目标及 PawnIO 资源。
- [v0.9.6 第三方声明](https://github.com/LibreHardwareMonitor/LibreHardwareMonitor/blob/v0.9.6/THIRD-PARTY-NOTICES.txt)：其中 PawnIO.Modules 列出 LGPL 2.1 声明；不据此推断独立驱动的全部许可义务。

## 6. 后续 Mac 探测报告最小格式（当前不创建工具）

后续获准开发时先用于本机 Mac；Windows 若另行启动可复用报告语义，不提前实现 Windows 探测。

每个对象/指标记录：环境与工具版本、脱敏对象标识、指标版本、来源、统计范围/单位/方法、权限与组件条件、实际样本时间/窗口、值与质量状态、错误原因、验证步骤和结果。

原始样本不足时保持“未验证”；来源未实现时保持“尚未实现”。精确型号可记录，序列号、产品 ID、用户路径、命令行和原始访问路径默认不进入共享诊断报告。报告仅写本机，由用户检查后自行决定是否提供，不自动上传。
