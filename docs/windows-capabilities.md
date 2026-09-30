## Windows 来源能力记录

来源验证与产品集成分开记录。以下为普通权限下的本机结果，不代表其他硬件/驱动兼容性，也不代表基础版本全部完成。

| 指标 | 来源与当前结果 | 产品状态 |
| --- | --- | --- |
| CPU 总/逐逻辑处理器 | sysinfo 0.30.13，原生测试读到 24 个逻辑处理器，原生 UI 有动态读数 | 已接入，首次预热为 null |
| 系统 RAM | sysinfo Windows 内存接口；总量与当前可用量分开，已用 = OS 总量 - 可用 | 已接入；不表示安装 RAM/硬件预留明细已完成 |
| 进程内存 | K32GetProcessMemoryInfo WorkingSetSize；使用与创建时间相同的只读句柄；读取失败为 null，真实零保留为零 | 已接入进程列表；普通权限覆盖范围仍需完整验收 |
| 进程 CPU | GetProcessTimes 的 kernel + user 时间差分，按实际单调时间窗与逻辑处理器数归一化；首次/身份变化/无有效窗口为空 | 已接入；创建时间以字符串传输防止精度丢失 |
| GPU 引擎 | 英文 PDH GPU Engine 通配计数器；短时间窗读到数百个有效进程/引擎实例 | 已接入；按适配器和引擎关联，首次预热不补零 |
| GPU 内存 | PDH GPU Adapter Memory 的 Dedicated Usage/Shared Usage 可读，可与一个 AMD DXGI 适配器关联 | 已接入专用/共享全局使用量；仍需同窗口对照，不使用本进程预算 |
| 物理磁盘吞吐 | PDH PhysicalDisk 与只读设备句柄关联，本次实测两块 NVMe 都有有效读写数据，未关联实例数为 0 | 已接入实时读写与合计吞吐历史；不把计数器数量当物理盘数量 |
| 盘卷容量与映射 | MSFT_Disk/MSFT_Volume 字段白名单，IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS 映射 | 两块 NVMe、4 个卷实测通过，含一个无盘符卷；跨盘/未知卷在独立区域展示 |
| 设备历史身份 | 随机匿名注册表；同一次运行依靠持有的只读设备句柄检查连续性 | 重启建立新序列，旧序列保留并可选择；不靠型号/容量猜配 |
| 进程存储 I/O | 已检查锁定版 sysinfo Windows 源码，内部使用 GetProcessIoCounters | 不作为纯存储来源；内核磁盘 ETW 能力探针返回权限错误 5，未启动会话 |
| 温度、通电、健康 | 本轮没有验证 Windows 传感器接口 | 未实现/未验证，未安装驱动 |
| 结束进程、自启 | Windows 端明确拒绝/禁用；保留 Mac 原有代码路径 | Windows 未实现，不以强制终止替代 SIGTERM |

本机 CIM 枚举到 Radeon 890M 和三个虚拟显示适配器。DXGI 实际返回五个条目，其中四个带 AMD vendor 信息，一个标识为软件适配器；只有一个 AMD 条目在此次探针中正确关联到动态计数器。不能由这些条目数量推断存在四块物理 AMD GPU，也不能只凭 software/remote 标志判定其他三个条目的身份。后续 W4a 必须补足适配器分类与关联验证。

只读探针的启动命令如下。它不创建应用历史、ETW 会话、自启项或驱动，只输出允许的汇总字段，不输出原始计数器实例名、PID、LUID、挂载路径或异常原文。

```powershell
python -X utf8 scripts/probes/windows_readonly.py --samples 3
```

探针使用 DOUBLE、NOSCALE、NOCAP100，检查每项有效状态并限制缓冲和枚举数量。GPU 的候选总利用率按进程/引擎归组后取最忙引擎；聚合超过 100%、重复实例或缺失数据不截断成一个看似有效的百分比。此算法已通过合成测试，但仍需本机其他程序活动与系统工具对照，不能把短探针结果写成 V-04/V-05 已通过。

官方口径：[PDH 格式化计数器数组](https://learn.microsoft.com/en-us/windows/win32/api/pdh/nf-pdh-pdhgetformattedcounterarrayw)、[进程全部 I/O 统计](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-getprocessiocounters)、[进程 GPU 预算接口](https://learn.microsoft.com/en-us/windows/win32/api/dxgi1_4/nf-dxgi1_4-idxgiadapter3-queryvideomemoryinfo)。


### W3 本机支持边界

本次实测为 ZHITAI TiPlus7100 2TB 与 CT1000P3PSSD8，两者 BusType 均为 NVMe。C、D 和一个无盘符卷关联前者，E 关联后者。早期探针曾返回三个性能实例，那是当时的计数器清单，不能作为当前物理盘数量。重启测试已确认旧样本数不变、新序列独立写入，原生界面可选择重启前曲线。

当前稳定身份关联仅用于代码明确分类的固定介质，实际验证只覆盖上述 NVMe。SATA/ATA、USB、真实热插拔、同型号多盘与 Storage Spaces 仍不能标为实机通过。虚拟或未知设备不会冒称已确认的物理盘；未确认身份时不使用盘号作为历史后备键。跨盘卷已有结构/合成测试，不代表复杂池底层物理拓扑已全部展开。

新增 Windows 依赖为 wmi 0.18.4（关闭默认 chrono 功能，MIT OR Apache-2.0）和 uuid 1.26.1（v4，用于本机生成随机系列 ID）；仅调用只读 WMI 查询和必要 Win32 读取接口。WMI 的 windows/windows-core 需保持同一兼容系列；本项目锁文件对齐 windows 0.61.3 / windows-core 0.61.2，未升级 Tauri 或安装服务/驱动。


2026-09-18：Windows 同一数据目录重复启动已支持通知现有实例恢复窗口；本机已验证关闭到托盘后再次启动，原窗口恢复、第二进程退出且没有新增采集段。不同目录通过文件锁和独立通知保持隔离。并发冷启动、跨版本和跨会话尚未单独实测。


2026-09-21 首版增强范围：用户选择将进程实际磁盘 I/O 和更多温度指标纳入首版，接受另行设计需授权的采集组件。普通权限补充探针读到两个 NVMe 与一个 USB 的 MSFT_Disk/MSFT_PhysicalDisk 健康状态；Storage Reliability Counter 查询返回 CIM 2（访问拒绝），未取得温度、磨损或通电值。此轮只做范围和来源核验，未集成功能或安装组件。具体目标、权限方案与执行门槛见 [首版增强范围](plans/2026-09-21-windows-enhanced-v1-scope.md)。


2026-09-23：E1 普通权限通信基础已实现，协议和真实双进程管道验证通过；主界面未接入增强来源，进程磁盘 I/O、温度和增强健康仍未实现。见 [E1 通信验收](windows-enhanced-ipc.md)。


2026-09-23：E2 一次性只读探针已准备。普通权限磁盘健康查询成功，可靠性计数器返回 80041003，ETW 返回 5；管理员实测尚未授权执行。运行范围和已构建程序散列见 [E2 探针说明](windows-enhanced-probe.md)。


2026-09-23 管理员单次验证：ETW 五秒读取 26 个、写入 215 个事件，无报告丢失，会话停止和消费者退出成功。进程归属与速度尚未实现。Storage Reliability Counter 直接枚举不再拒绝访问，但成功返回空行；温度/磨损/通电仍未取得，后续验证关联查询路径，不能标为硬件不支持。详见 [管理员证据](windows-enhanced-probe-admin-evidence.json)。


2026-09-23 补充：按磁盘调用 PS_StorageCmdlets.GetStorageReliabilityCounter 的单次管理员验证成功。盘 0/1 温度分别返回 56/41°C，盘 2（USB）返回 0°C，暂不视为有效温度；三者 Wear=0、PowerOnHours=null。前两者为可接入候选，尚未完成多次及独立对照；不把 Wear=0 转为寿命 100%。这推翻的是“直接类枚举为空即可代表没有读数”的判断，不改变普通权限下仍受限的事实。见 [证据](windows-storage-method-admin-evidence.json)。


2026-09-24：E3 进程磁盘 I/O 归属计算模块已完成合成事件测试，覆盖 PID/TID 复用、缺事件、跨窗口、超限和超时；全体 Rust 测试 103 通过。真实 ETW 生命周期与请求事件解码尚未接入，进程磁盘排行仍不可用。见 [归属模块验收](windows-process-disk-attribution.md)。


2026-09-24：新增 TDH 字段解码与生命周期事件分类，Windows 原生 TDH 合成样本验证通过。真实进程创建标记、基线交接和排序尚待实现，进程磁盘排行仍未启用；本轮未提升权限或启动跟踪会话。


2026-09-24：已加入持有句柄的进程/线程身份核验、有界排序、限定集合的原子基线交接；普通权限自进程/线程测试通过。全系统基线枚举、时钟映射及实时生命周期应用尚未接入，仍不能启用真实进程磁盘排行。当前 Rust 测试 115 通过。


2026-09-24：新增 QPC/FILETIME 区间映射和有界系统快照枚举。普通权限快照取得 246 个核实进程、7933 个核实线程，同时明确记录覆盖缺口；已验证集合可安装基线，但全系统实时事件交接尚未完成。真实进程磁盘排行仍未启用，未再次提升权限。


2026-09-24：归属、排序和基线已接入带轮次隔离的 Pipeline 状态机，普通权限候选基线实测安装/停止通过。生命周期变化仍保守要求重建；ETW 回调实时监督与全量快照尚未接入，进程磁盘排行未启用。


2026-09-30：磁盘页增强采集的真实启用、读数更新和停止流程通过；两块 NVMe 温度与 Windows 健康状态已实际显示，通电未知值和 USB 无效温度显示 —。采集组件停止后退出。温度历史尚未接入。见 [界面验收](windows-storage-enhanced-ui.md)。


2026-09-30：磁盘温度历史与最近一小时图表已接入代码，新增前后句柄/匿名系列核对和有界独立写入。编译及针对性检查通过，真实授权采集后的温度入库和曲线仍待验证；不将代码接入标为实机通过。

2026-09-30 后续：修复存储协议版本检查遗漏后，授权界面采集已实际保存两块 NVMe 各 6 个温度样本，覆盖 29 秒。停止后曲线仍可查看，范围为 54–56°C 和 41°C；匿名 UID 与既有注册表相符，USB 无效温度未入库，组件与隔离主界面均已退出。温度历史实机流程通过；CPU/GPU 温度和进程磁盘 I/O 尚未完成。见 [历史证据](windows-temperature-history-evidence.json)。

2026-09-30 GPU 来源：普通权限下使用现有签名有效的 ADLX 1.5.0.124 运行库，Radeon 890M 三次返回边缘温度 61°C，LUID 与 DXGI 唯一匹配。未安装新驱动、未请求 UAC、未写应用历史，GPU UI/历史尚未接入。CPU 的 LHM/PawnIO 候选已完成初步代码核对，实际组件准备与本机读取仍待推进。见 [温度来源与交付边界](windows-temperature-sources.md)。
