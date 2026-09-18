## 首批 Windows 数据契约

本轮为成套前后端变更。命令名称保留，返回结构有明确调整；旧前端不能直接搭配本轮后端。Rust DTO 位于 `monitor-app/src-tauri/src/model.rs` 和 `processes.rs`，TypeScript 类型位于 `monitor-app/src/hardware.ts`。

| 位置 | 契约 |
| --- | --- |
| SystemInfo.cpu / memory | 可以为 null，不为缺失来源填 0 |
| SystemInfo.gpus | 适配器数组，取代原单一 gpu 对象；每项 object_id 用于历史查询；Mac 既有对象保留 gpu0 |
| SystemInfo.source_states | 按 cpu、memory、gpu、storage、disk_throughput 表达状态，支持 ok/warming_up/not_implemented/unverified/unsupported/not_applicable/permission_required/error/stale |
| SystemInfo.source_meta | Windows 已注册来源提供上次成功观察时间、单调时钟计算的年龄、实际配置周期和来源序列号；失败不会刷新旧成功时间 |
| CPU | 总/每逻辑处理器百分比；物理核数允许 null；Windows 首次建立基线后才给占用率 |
| 系统 RAM | bytes；总量为 OS 可使用总量，已用 = 总量 - 当前可用；不是安装总量，也不是进程工作集求和 |
| 进程内存 / CPU | 允许 null；Windows 内存标注为工作集，CPU 按整机逻辑计算容量归一化；预热、无身份或无有效窗口不补零 |
| ProcessInfo.start_marker | 字符串或 null；Windows 保留完整 FILETIME，避免 JavaScript Number 精度损失；Mac 秒级标记也改为字符串 |
| terminate_process 参数 | pid + startMarker（字符串或 null）；后端校验转换；Windows 路径不发送任何终止操作 |
| get_runtime_info | 新增平台、功能可用性、主实例标志和采样间隔元数据；UI 不用 userAgent 猜平台 |
| get_system_info | 仍只读缓存，不触发硬件扫描 |
| get_history | 仍为 [Unix 秒, 数值][]；白名单暂未增加；只写本轮实际有效来源 |
| get_system_status.history_lost_batches | 当前运行中因队列已满或写入失败而未保存的历史批次数；单独提示可能缺口，不补造样本 |

Windows 当前写入 CPU 总量、逐核、系统内存，以及已确认匿名身份的磁盘合计吞吐历史。GPU 未实现时不写相应样本。Mac 来源读取失败时对应对象为空、状态为 error，其余成功来源可以发布；不会再用拓扑空列表掩盖整类来源失败。

2026-09-15 的 Windows 调度已改为每个来源一个固定线程，CPU 与 RAM 独立采集，最多一个在途调用。SQLite 写入使用独立线程和容量为 16 批的有界队列。队列满或写入失败会累加未保存批次数，实时缓存不等待数据库。历史批次只包含这次来源产生的新值和观察时间，不重写组合快照中的其他缓存值。

Windows 已实现按来源周期判断过期、配置变化唤醒等待线程，以及退出时唤醒并有界等待工作线程。无法取消的系统调用不会被假称已取消，也不会生成替代线程；超过关闭期限会记录未返回线程数。已用合成阻塞来源和写入器验证这些行为。GPU 真实来源仍未接入；W3 已增加存储与磁盘吞吐两个独立来源。Mac 暂保留上一阶段的阻塞采样适配器，其独立来源拆分和原生回归仍待执行。历史继续使用秒级时间戳，500 ms 采样与后端抽稀问题保留在 W5b。

Windows 数据目录由 Tauri 与 MONITOR_DATA_DIR 决定；原生窗口在解析后端根之后创建，WebView 存储位于其 webview 子目录，debug 日志位于 logs 子目录。唯一写入锁由应用状态持有；同目录第二实例提示已有实例后退出，不伪装成独立查看器。自动激活主实例仍属于 W5a。


2026-09-15 的进程采集进一步收窄：sysinfo 只负责 Windows 进程清单，创建时间、CPU 累计时间和工作集由同一进程句柄读取。CPU 差分先比较完整创建标记，再计算实际时间窗；退出、重用、计数回退和无效窗口不产生假速率。Windows 默认枚举不请求命令行、环境变量、可执行路径或一般 I/O 字段。`total_readable` 为兼容保留的字段名，UI 改称已枚举进程，各项指标是否可读仍由可空值表达。

进程统计接口依据：[GetProcessTimes](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-getprocesstimes)、[GetProcessMemoryInfo](https://learn.microsoft.com/en-us/windows/win32/api/psapi/nf-psapi-getprocessmemoryinfo)。


## W3 存储契约

- SystemInfo.windows_storage 包含系统磁盘设备、卷、卷到磁盘编号的关系、历史序列清单及注册表/卷来源状态。Mac 的原存储字段保留，Windows 不构造 APFS 容器。
- SystemInfo.windows_disk_throughput 包含每个设备的读/写 B/s、匿名系列 ID、状态和未关联实例数。UI 同时匹配当前设备编号与匿名 ID，不能仅凭盘号套用旧读数。
- 静态磁盘清单约 30 秒刷新；卷容量和身份检查最低 5 秒；吞吐按前后台动态周期采集。短时间内移除后又重连、复杂桥接设备等仍需后续实机验证。
- device-registry.json 只保存随机匿名 ID、名称、容量与系列创建时间；运行时盘号和原生句柄不落盘。注册表损坏不静默覆盖，保存失败不发布未持久化的新 ID。
- 重启没有前一进程持有的设备句柄，故建立新系列；旧系列可在存储详情的历史连接选择器中查询。显示名称相同不意味着确认是同一设备。
- 只有身份已确认、读写均有效且非预热时，才将二者合计写到 disk.throughput；单位为十进制 MB/s。温度未接入，不写磁盘温度零值。
- 卷容量来自 MSFT_Volume 的 Size/SizeRemaining，映射由卷 extent 查询提供。跨盘和未确认归属的卷在独立区域展示一次，不分摊到某一块盘；文件系统使用量与物理盘容量不混作同一个百分比。

W3 不包含完整物理分区列表或 Storage Spaces 底层成员展开；未知层级保留为未知。历史返回点的后端抽稀、秒级时间戳及完整七天留存仍按 W5b 继续处理。


## W4 GPU 契约

GpuInfo.windows_memory 区分专用使用量与共享使用量，并提供未确认适配器条目数和本地历史系列。memory_used_bytes 在 Windows 为两者合计；memory_allocated_bytes 在 Windows 为 null，仅 Mac 保留已验证的驱动分配量语义。UI 不把集显专用统计解释为独立 VRAM 芯片，不生成没有正确分母的占用率。

GPU Engine 按适配器、物理节点和引擎归组后取最忙引擎；无效项、重复项或超出范围的聚合不强行截到 100%。当前仅发布利用率、专用用量和共享用量均取得有效数据的适配器，其余条目保持未确认。本机之外的多 GPU/多节点设备尚未验证。

GPU 使用独立的 gpu-registry.json 随机系列。文件中的 size_bytes 是 DXGI 静态容量提示，只用于连接描述，不能用作 UI 显存占用率分母；LUID 和进程实例名仅在采集线程中关联，不落盘。应用重启或适配器清单失效时保守建立新系列；完整重启 UI 验证仍待执行。


W5b 首批修复后，512 MiB 软预算统计 monitor.db、monitor.db-wal、monitor.db-shm。缺少可选 sidecar 按不存在处理；主库缺失或其他大小读取错误会阻止新写入并报告存储错误，不默认为零。该预算在批次写入前检查，仍允许单批越过软阈值，不承诺逐字节硬上限。


## 历史显示协议 v2

get_history_v2 返回 version=2、segments、input_points、downsampled、omitted_segments、aggregated、continuity。每个点包含 t、value、min、max、count、granularity_secs、first_ts、last_ts。前端沿用后端分段，不按展示点之间的距离再次猜测缺口；只显示返回记录，预算省略情况明确提示。此协议不修改数据库结构。

原始记录按秒合并：value 为均值，min/max/count 保留同秒范围及数量。连续性按局部记录间隔推断；时间间隔上下文不足时显示独立点。粗粒度汇总点始终独立，不声称桶内或跨桶连续。查询边缘可能包含整个相交汇总桶；时间键相同时较细粒度优先，不等于对所有重叠时间范围进行了逐样本去重。精确来源质量、毫秒级时间戳和中断边界需要后续持久化方案。


Windows CPU、GPU、磁盘吞吐采样增加双时钟间隔检查。系统时间回拨、与单调时钟的间隔差大于 2 秒，或间隔大于 max(前后周期 × 3, 15 秒) 时，本次跨间隔差分不可发布；重新预热后才继续写入有效指标。此规则不产生精确的电源事件或历史缺口元数据，也不能识别全部短暂停。


## 原始记录连续段扩展

schema_version=2 新增可空 metric_samples.segment_id；旧记录保持 null，旧版写入也可继续省略此列。Windows 使用随机采集段标记，不包含硬件标识。每个来源仅在尝试序号相邻、无时钟边界且上次写入事务成功时延续段；失败批次不确认成功。get_history_v2 增加 recorded_boundaries 和点级 segment_id，仍保持协议版本 2 的向后兼容附加字段。不同段不连线，同秒不同段不合并，段内仍可按间隔保守分开。

此扩展不保证所有中断均可识别。来源部分成功导致的单设备缺测仍主要依赖时间间隔，10 秒及 60 秒汇总未保留段标记，聚合后只展示独立范围点。进程 CPU 差分在轮询暂停或双时钟异常时重新预热，进程即时内存不因此清空。


## 语言与采样设置

桌面语言以 settings.json 为持久化来源；monitor-language 仅用于从旧 WebView 缓存迁移，明确已保存值优先，迁移成功才删除缓存。system 仍表示原应用默认中文。主题策略保持不变。set_language 和 set_sampling_intervals 在后端锁内读取最新配置、只改指定字段并保存；损坏文件拒绝局部写入。旧 set_settings 保留全量兼容语义，新前端不再以旧草稿整体覆盖配置。


GPU 新拓扑发布条件增加稳定窗口：至少 3 秒、连续 3 次有效观测。来源周期较慢时以三次观测为准；计数器或拓扑失效重新等待。等待不改变匿名身份原则，不合并旧连接，也不覆盖已有历史。整个采集来源失败的历史分段机制保持不变。
