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
