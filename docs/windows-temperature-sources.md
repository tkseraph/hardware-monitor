# Windows CPU/GPU 温度来源（2026-09-30）

## GPU：普通权限来源已确认

本机已有 C:\Windows\System32\amdadlx64.dll，版本 1.5.0.124，Authenticode 有效，签名者为 Microsoft Windows Hardware Compatibility Publisher。没有安装或替换驱动。选择与运行库一致的官方 ADLX v1.5，锁定提交 d9f04a9bba022d6cf6333f005dd540b4ad19fb63；源码只在开发工具目录读取，SDK 头文件和供应商二进制未复制进项目。

新增限定程序 scripts/probes/windows_gpu_temperature.cpp 与构建/运行入口 windows_gpu_temperature.ps1。程序不接收运行参数，只从固定 System32 路径加载 ADLX，拒绝不同运行库版本；入口运行前核对驱动库签名。进程在普通权限下受 15 秒外部超时限制，输出不超过 32 KiB。代码仅取得 GPU 清单、LUID、指标支持标志及当前温度，不启动指标历史追踪，不调用调频、风扇、功耗或写控制接口。所有引用在 ADLXTerminate 前释放，运行库最后卸载。

一次运行、三次间隔 2 秒的观测，Radeon 890M 均返回 61°C。IsSupportedGPUTemperature 为 true，IADLXGPU2 的 LUID 与 DXGI AMD 适配器唯一匹配；热点支持标志为 false，热点值为 null，没有用封装或边缘温度补值。成功退出表明 ADLXTerminate 返回成功，未残留探针进程。详细结果见 [来源证据](windows-gpu-temperature-evidence.json)。这不是独立工具校准，也不代表已完成产品 UI/历史接入。

官方定义 GPUTemperature 为 GPU 晶粒边缘平均温度，单位摄氏度；GPUHotspotTemperature 为结温传感器中的最大值，需保持独立指标。依据 [ADLX v1.5 性能接口](https://github.com/GPUOpen-LibrariesAndSDKs/ADLX/blob/v1.5/SDK/Include/IPerformanceMonitoring.h)。未来关联沿 LUID 与现有 GPU 匿名系列核对，不能按名称或清单第一个对象猜配。

## SDK 的交付边界

已读取锁定 SDK 中 8 页 ADLX SDK License Agreement.pdf，SHA-256 C62178B9D30F75450088C5238193644B45AB5A23442F70F5A3F6B09B951180A6。它是 AMD 自定义 SDK 条款，不能以 GitHub 托管推定为 MIT 或任意再分发。

条款第 2 节涉及内部使用和作为产品一部分的目标代码分发，第 3 节规定终端协议要求，第 4 节限制 SDK 材料的公开分发及使其受特定自由软件许可约束。这里记录工程交付条件，不替项目决定整体许可证。当前只保留自编限定探针；产品打包前需选择符合项目分发方式的独立接口实现或落实所需协议，不把整个 SDK 或现有驱动 DLL 加进公开仓库。原文：[ADLX SDK 许可](https://github.com/GPUOpen-LibrariesAndSDKs/ADLX/blob/v1.5/ADLX%20SDK%20License%20Agreement.pdf)。

## CPU：候选路径仍需组件准备

已固定 LibreHardwareMonitor v0.9.6，提交 3d331e3370efb858411f19511373eff65a218701。CpuGroup 将 AMD 家族 17h/19h/1Ah 路由到 Amd17Cpu；该实现通过 AMDFamily17 PawnIO 模块读取 SMN 温度寄存器，并区分 Tctl/Tdie 与偏移。这只能确认候选代码路径，不能证明 HX 370 在本机的传感器已可读。

本机未发现 PawnIO 的服务注册项或卸载项，也未发现 dotnet 命令。本轮没有加载 LHM、打开 PawnIO 设备或安装组件。LHM 的 PawnIo.Execute 在设备未加载时会返回零缓冲，因此将来必须核对 IsLoaded 和来源成功状态，不能把该回退值记成真实温度。

仅把 IsCpuEnabled 设为 true 还不足以保证“只读封装温度”：Amd17Cpu 构造过程会建立 RyzenSMU，Update 也读取其他传感器。后续主机需限制入口与调用范围，关闭主板/EC/SMBus/控制器扫描和所有调节控制，核实本型号传感器含义，再接入产品。依据 [CPU 路由](https://github.com/LibreHardwareMonitor/LibreHardwareMonitor/blob/v0.9.6/LibreHardwareMonitorLib/Hardware/Cpu/CpuGroup.cs)、[CPU 实现](https://github.com/LibreHardwareMonitor/LibreHardwareMonitor/blob/v0.9.6/LibreHardwareMonitorLib/Hardware/Cpu/Amd17Cpu.cs)、[PawnIO 加载与回退](https://github.com/LibreHardwareMonitor/LibreHardwareMonitor/blob/v0.9.6/LibreHardwareMonitorLib/PawnIo/PawnIo.cs)。

LHM 库工程声明 MPL 2.0，PawnIO.Modules 的第三方声明为 LGPL 2.1；独立 PawnIO 驱动/安装器的来源、版本、签名、许可、服务与卸载行为仍须单独核对。这不代表已批准安装。需先完成可审阅的具体清单与限定主机，再请求该次安装/运行授权。

## 接下来的实现

1. GPU：落实 SDK 交付选择后接入隔离读取、LUID/匿名系列关联、实时温度和历史；当前显示对象应称边缘温度，热点保持未知。
2. CPU：准备限定主机及 PawnIO 组件清单；授权前只做代码与依赖准备。
3. 进程磁盘 I/O：继续现有 ETW 事件归属与丢失状态处理，再接到进程页；不以全 I/O 计数器或磁盘总吞吐替代。

磁盘温度历史已经实机接通，见 [磁盘验收](windows-storage-enhanced-ui.md)。不再重复该温度保存流程；后续只针对新接入的功能进行必要检查。
