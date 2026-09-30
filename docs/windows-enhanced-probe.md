# E2 只读来源验证程序（2026-09-23）

状态：程序已准备、普通权限已测试；管理员验证待用户授权。该程序不等于可部署的增强采集服务，也不代表已完成 ETW 进程归属或 CPU/GPU 温度集成。

## 可审阅的运行对象

- 源码：`monitor-app/src-tauri/src/enhanced/probe.rs`。
- 协调程序：`monitor-app/src-tauri/examples/enhanced_readonly_probe.rs`。
- 本机生成文件：`monitor-app/src-tauri/target/debug/examples/enhanced_readonly_probe.exe`。
- SHA-256：`4F849BDA376B255A204AD2C14FAA5CBF608B243E30669F60824AE60E560ED3EC`。
- 签名状态：NotSigned，本项目本地开发构建，不是第三方下载程序或正式安装包。
- 手动运行不带参数；程序不自动提升权限。管理员运行须由用户明确授权并自行确认 UAC。

## 实际操作

1. 启动自己的存储子进程，使用 WMI 白名单查询系统盘号、总线类型、Windows 健康状态，以及 DeviceId/Temperature/TemperatureMax/Wear/PowerOnHours。每个查询最多 32 行；非数字设备 ID 不输出原值，只保留未知槽位，且槽位不作为已确认硬件身份。
2. 存储查询在独立进程中执行，协调程序最长等待 15 秒。stdout 最多接收 32 KiB，超时或输出超限只回收自己创建的进程。
3. 启动自己的 ETW 子进程，以本次专用名称请求内核磁盘实时会话；缓冲目标为 2～4 个 64 KiB 块，观察 5 秒。仅按 provider GUID 与 Read/Write opcode 统计事件数量，回调不读取 UserData，不记录文件名、PID、内核地址或事件原文。
4. ETW 子进程停止自己的会话、关闭消费者，并有界等待消费者线程；协调程序同样有 15 秒上限。超时或停止失败时，协调程序尝试按本次名称回收该会话。报告保留停止、关闭、丢事件和消费者返回状态，不能把 StartTrace 成功当作全部采集成功。
5. 报告以 create_new 写入当前用户临时目录 `hardware-monitor-e2-*.json`，包含上述白名单字段及状态，不写应用数据库。协调程序退出后不再采集。程序可写该报告及创建短期 ETW 会话，不是“完全无副作用”。

该限时开发探针使用一次性会话名称以防误操作其他会话。生产组件应另行设计稳定的所有权标识、并发限制和崩溃恢复流程；若协调程序本身被强制终止或系统崩溃，仍可能留下会话，需要人工核查。不要并发运行多份探针。成功报告仍须核对 stop_code 与消费者退出状态。

不安装服务或驱动、不注册自启、不调整电源或系统安全设置、不终止用户应用、不修改磁盘数据。运行时不会尝试改变令牌权限；管理员模式只使用用户授予的进程权限。CPU/GPU 传感器库、PawnIO 和 ADLX 不在此程序中加载。

## 已完成验证

默认 Rust：94 通过、0 失败、8 忽略；严格 Clippy 与构建通过。测试覆盖未知/零值、64 位通电值、过滤额外标识字段、ETW 配置边界及拒绝清理非本组件名称。本阶段复用了 E1 协议测试，但该离线探针没有通过 E1 管道向应用传输数据。

普通权限实测：两块 NVMe 和一块 USB 的 HealthStatus=0；Storage Reliability Counter 枚举阶段返回 HRESULT 80041003；ETW StartTrace 返回 5，未启动会话、未消费事件。早期实现将枚举阶段错误归为 decode_error，已修复为保留查询 HRESULT；没有将访问拒绝当成硬件不支持。

字段是来源能力证据，尚未做磁盘身份关联、单位异常过滤、字段宽度/哨兵值核验或产品展示。通电与磨损值即使读取成功也必须在后续独立核实。ETW 此次只验证事件到达，不计算每进程读写速度。

## 授权后验收与下一步

单次运行通常数秒，两个子进程合计等待上限约 30 秒，另含启动和停止开销。授权后核对：可靠性字段是否可读；是否出现真实读写事件；事件丢失/缓冲丢失；会话停止和消费者退出；所有子进程退出；报告完整。

若仍拒绝访问，不继续扩大权限或安装驱动。先保留错误与来源限制。若通过，再进入进程生命周期归属、真实字节数解码和全量快照设计；LHM/ADLX 兼容性与依赖审核仍是独立的后续 E2 工作。

依据：[Microsoft StartTrace](https://learn.microsoft.com/en-us/windows/win32/api/evntrace/nf-evntrace-starttracea)、[Storage Reliability Counter](https://learn.microsoft.com/en-us/windows-hardware/drivers/storage/msft-storagereliabilitycounter)。


## 单次管理员验证结果（2026-09-23）

用户明确授权后，校验上述 SHA-256 一致，通过 UAC 启动一次程序。此次授权已使用完毕，没有自动重复提升权限。

ETW StartTrace=0；五秒内读取事件 26、写入事件 215；EventsLost=0、RealTimeBuffersLost=0；StopTrace=0、CloseTrace=0、ProcessTrace=0，消费者线程已回收。运行后探针进程已退出。该结果确认管理员权限下磁盘事件可以到达，不证明已实现每进程归属、字节数或准确速度排行。

MSFT_Disk 健康查询成功，两块 NVMe 和一个 USB 设备返回 HealthStatus=0。可靠性计数器直接类枚举返回成功但 rows=[]，没有温度、磨损或通电值。普通权限的访问拒绝已解除，但此枚举路径未提供记录；不得解释为全部磁盘没有传感器。下一步应准备按磁盘关联/提供程序方法的查询与必要的只读存储查询，再单独安排需要权限的运行。

本轮未安装驱动/服务，不写应用历史；没有对 CPU/GPU 温度库进行验证。脱敏结果见 [管理员验证证据](windows-enhanced-probe-admin-evidence.json)。

## 按磁盘读取路径（2026-09-23，待单次管理员验证）

检查本机 Windows StorageCmdlets.cdxml 后确认，现行 Get-StorageReliabilityCounter 调用 PS_StorageCmdlets.GetStorageReliabilityCounter，并传入 Disk 或 PhysicalDisk 实例。直接枚举类返回空行不代表该方法也返回空行。

新增 --storage-method-only 模式：枚举最多 16 个 MSFT_Disk 的 Number/ObjectId，将当前磁盘实例作为固定只读方法参数；只输出盘号、方法状态和原有可靠性字段。ObjectId 仅在 WMI 调用内使用，不写入报告。构造输入实例的 put_property 只设置调用参数，不写磁盘或提供程序属性。非零方法返回码不发布读数，空结果和未知槽位明确保留。

该模式只运行一个存储子进程，最长等待 15 秒，超时只回收自身子进程；不创建 ETW 会话、不安装驱动、不修改磁盘数据。报告以 hardware-monitor-storage-method-*.json 写到本机临时目录。运行参数必须为 --storage-method-only，否则默认模式仍包含 ETW 检查。

11 项增强模块测试通过，严格 Clippy 和探针构建通过。普通权限按三块系统磁盘分别查询，均返回 80041003。管理员读取结果尚未知。上一轮单次授权已经使用，本轮需要新的单次授权。

本次程序 SHA-256：C477840AFB086C4465156BDE6957940914A40E8B9674BF102F2A5B2F2626C7DE。尚未重新运行管理员验证；旧管理员报告只适用于先前构建和直接枚举路径。


## 按磁盘读取的管理员结果（2026-09-23）

用户明确授权后校验 C477840A…2626C7DE 对应的完整散列，单次以 --storage-method-only 运行，探针及子进程已退出。未启动 ETW 或安装任何组件。

| 本次系统盘号 | 方法返回码 | Temperature | TemperatureMax | Wear | PowerOnHours |
| --- | --- | --- | --- | --- | --- |
| 0 | 0 | 56 | 90 | 0 | null |
| 1 | 0 | 41 | 83 | 0 | null |
| 2 | 0 | 0 | 0 | 0 | null |

与直接枚举返回空行不同，PS_StorageCmdlets.GetStorageReliabilityCounter 的按盘读取实际提供了计数器。两块 NVMe 的当前温度字段可作为后续产品接入候选；这是单次来源读数，尚未与独立工具同时间窗对照。USB 的 0°C/最大值 0 不作为已验证有效温度，Wear=0 也不推算为剩余寿命 100%。通电时间均为空，不填零、不按运行时间补算。TemperatureMax 保留原始字段，不能自动解释为实测历史最高温度。

盘号及 DeviceSlot 只用于此时查询，产品接入仍须确认设备句柄与匿名历史身份映射，并安排多次观测和单位/语义验证。本次单次授权已使用完毕。脱敏结果见 [存储方法验证证据](windows-storage-method-admin-evidence.json)。


## E3 实时管线限时入口（2026-09-28，待单次管理员授权）

程序：`monitor-app/src-tauri/target/debug/examples/enhanced_readonly_probe.exe`；参数必须是 `--pipeline-only`。SHA-256：`05A0549BAB97A1B8B7978C39D0852F106967D7B06FE6C7DC17C76A9C4EF72913`；NotSigned，本地开发构建。

此模式新增 trace_run.rs：启用磁盘发起/完成及进程、线程生命周期事件，使用 RAW_TIMESTAMP 与 QPC；回调只将白名单字段解码到容量 8192 的非阻塞队列，丢弃和解码错误可见。最多保留 16 类失败事件的 provider family/opcode/version 计数，报告不含原始事件、IRP、PID、线程号、命令行或文件路径。

协调线程枚举只读进程/线程身份，观察窗口约 10 秒，每轮有界转移队列、确认时钟及查询 ETW 丢失计数，使用延迟 2 秒的排序水位；候选基线延后交接，保留切换后事件。收到不能安全处理的事件或生命周期变化时允许结果失效，不自动宣称完整排行。2 秒仅为本次诊断参数，不能保证所有机器事件都在该延迟内到达。

结束时停止自身会话、关闭消费者、有界等待线程；回调上下文由 Arc 保持到消费者返回，避免超时后释放仍在使用的指针。外层独立子进程监督最长等待 25 秒，失败后回收自身子进程并尝试清理本次唯一会话。实际返回停止/消费者错误码，不能将启动成功视为退出成功。协调器本身崩溃的残留会话风险仍需生产级清理设计。

输出 `hardware-monitor-pipeline-*.json` 到用户临时目录，只有基线覆盖数量、状态、事件质量、观测到的子集汇总字节和停止结果。一直保留 systemwide_stream_ready=false；即使取得窗口，也不是整机完整排行验收。事件本身可能由系统携带额外字段，程序只读取白名单字段，不保存未读取的 payload。没有额外制造文件读写负载，不读取文件内容，不安装驱动/服务，不修改应用历史，不提升主 UI 权限。

普通权限实测：StartTrace=5，未启动会话。默认 Rust 125 通过、0 失败、9 忽略；严格 Clippy 与探针构建通过。新增测试覆盖队列溢出可见及无关 provider 不访问 payload。之前的管理员单次授权均已用完，本模式仍待新的单次授权。


## E3 实时管线管理员结果（2026-09-28）

用户单次授权后核验 05A0549B…4EF72913 的完整散列，按 --pipeline-only 运行。StartTrace=0；实际解码 23817 个事件，decode_errors=0，失败 schema 列表为空。身份快照核实 389/419 个进程（29 不可用、1 idle）与 9107/10936 个线程。

本次速率验收未通过：结束前状态 Invalid(Ordering)，ready window=0；本地 callback 队列丢弃 3189 个事件，尽管 ETW 本身 EventsLost/BuffersLost=0。字节汇总 0 只是没有有效窗口，不是磁盘空闲证据。当前报告无法确定每次丢弃是在运行中还是关闭缓冲排空期间，也未区分排序迟到与容量问题，后续需要分阶段统计与明确错误类型。

raw loss_observed=false 与最终 queue_dropped 不一致。代码中该布尔值仅在主循环检查，关闭阶段仍可能有回调，最终评估必须结合最终计数；本次按存在丢失处理，不能覆盖原报告伪装成功。后续修正最终汇总和停机排空策略。

StopTrace=0；CloseTrace=7007；ProcessTrace=0，消费者已 join。Microsoft 文档说明 ERROR_CTX_CLOSE_PENDING 表示异步关闭调用成功，等待缓冲内事件处理结束：[CloseTrace](https://learn.microsoft.com/en-us/windows/win32/api/evntrace/nf-evntrace-closetrace)。探针及子进程已退出；logman 会话查询退出码 0，未发现 HardwareMonitor-E2- 前缀会话。此轮没有安装驱动/服务、制造文件负载或写应用历史，未重复使用单次授权。

结果证据：[管理员管线报告](windows-etw-pipeline-admin-evidence.json)。下一步优先修复队列排空、排序诊断和最终丢失状态，不把本次事件解码成功扩大为进程排行已实现。
