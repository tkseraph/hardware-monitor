# CPU 温度组件安装与单次验证清单（2026-09-30）

限定采集程序已编译，尚未安装驱动或取得 CPU 温度。本机普通权限验证确认 Ryzen AI 9 HX 370 为 AMD 家族 1Ah、型号 24h，三次均返回“组件未安装”，温度为 null，没有显示零温度。CPU 界面和历史尚未启用。

## 本次待授权动作

安装已校验的官方签名 PawnIO 2.2.0，然后仅运行一次已准备的 CPU 温度程序：三次低频观测，间隔 2 秒，子进程最长 12 秒，输出不超过 16 KiB。安装和读取都可能出现 UAC，由用户自行确认。若安装器报告需要重启，先报告并等待用户安排，不自动重启。

这是新增的持久内核驱动安装，先前开发工具安装及单次磁盘读取授权没有覆盖它。不会在授权前执行安装器，或以“继续开发”代替此安装授权。

## 安装对象与系统影响

官方安装器：[PawnIO 2.2.0](https://github.com/namazso/PawnIO.Setup/releases/tag/2.2.0)。已从官方来源下载到 D:\Program\monitor\.tools\PawnIO_setup.exe，文件版本 2.2.0.0，SHA-256 为 1F519A22E47187F70A1379A48CA604981C4FCF694F4E65B734AAA74A9FBA3032。Authenticode 有效，签名者 namazso.eu / namazso，带 Microsoft 时间戳。

安装包已作为数据解包检查，没有运行其中的安装器或工具。提取的 AMD64 官方驱动 PawnIO.sys 为 2.2.0，SHA-256 FCA6E7D58B0CF38DBB913A2B9E532F48629145D395F454B16A9F58E97B8D3940；对应目录的 Microsoft Windows Hardware Compatibility Publisher 签名 catalog 有效，Windows SDK 签名工具以内核策略成功验证该驱动属于此 catalog。

| 对象 | 内容 |
| --- | --- |
| 驱动设备 | Root\PawnIO，SoftwareDevice 类 |
| 内核服务 | PawnIO，ServiceType=1，StartType=3（按需启动） |
| 驱动位置 | INF DIRID 13，由 Windows 放入 DriverStore；安装后核对实际 ImagePath |
| 默认应用目录 | %ProgramFiles%\PawnIO，预计包含库、工具、头文件和卸载器；安装后记录实际清单 |
| 访问权限 | INF 默认仅 SYSTEM 和已提升的管理员可访问 |
| 卸载入口 | Windows“已安装的应用”中的 PawnIO，以及该目录的 uninstall.exe |

驱动、设备和卸载注册项会在本软件退出后保留，可能被其他监控程序共享，不自动卸载或停止共享驱动。安装前未发现 PawnIO 服务、卸载项或默认目录；若安装前再次检查发现已有组件，先核对现有实例，不自动覆盖/升级。卸载按用户后续明确要求执行，并核对共享使用情况。

拟用安装参数为 -install -silent；不会使用 -unrestricted 或 -debuginfo，不更改驱动签名、内存完整性、设备访问权限或其他安全策略。若 Windows 拒绝该官方驱动，保留失败结果，不尝试绕过。官方发布说明指出安装可能返回需要重启的状态，见 [2.2.0 发布记录](https://github.com/namazso/PawnIO.Setup/releases/tag/2.2.0)。

## 程序的读取边界

复用 Rust 程序，不再引入额外 .NET 主机。独立 IOCTL 客户端只允许已核验的 AMD64 驱动散列、版本 2.2.0 和当前 CPU 家族/型号；确认只有一个物理处理器对象后，加载固定的已签名 AMDFamily17 模块，仅调用 ioctl_read_smn，输入偏移固定为 0x59800。

温度按寄存器字段报告为 Tctl（CPU 控制温度），不是已经校准的 Tdie。SMN 读取协议会先写 PCI 0x60 的选址，再读 0x64 的数据，这是一次温度读事务，不调用 MSR 写入、调频、功耗限制、SMU 邮箱、主板/EC/SMBus 扫描。全局 Access_PCI 互斥等待最多 100 毫秒；未获得锁、锁遗弃、返回尺寸错误或无效寄存器均报告失败。源不接受用户给定的模块、函数名、地址或设备路径。

模块取自官方 [0.2.2 发布](https://github.com/namazso/PawnIO.Modules/releases/tag/0.2.2)，与 LHM v0.9.6 内的副本逐字节一致，SHA-256 为 099DC01D6DB97EA997FEC4A461E191CC64B9D7CE47C9D2153C451C56C2ADCF50。其 4096 位 RSA 签名已用驱动源码公布的公钥离线验证。模块含有其他硬件入口，本软件没有调用或开放它们；不能把通用 PawnIO 驱动描述成只有温度读取能力。

驱动声明 GPL-2.0-or-later，附独立 device-IOCTL 客户端的许可例外；模块为 LGPL-2.1-or-later。固定模块、对应源代码、头文件与声明已保留于 vendor/pawnio-amdfamily17-0.2.2，并提供替换模块、更新预期散列及重建的路径。没有把驱动或其工具打包进主应用；整体应用许可证仍未选定。[驱动许可](https://github.com/namazso/PawnIO)、[模块源](https://github.com/namazso/PawnIO.Modules/tree/0.2.2)。

## 已准备的验证程序

程序：monitor-app/src-tauri/target/segments/x86_64-pc-windows-msvc/debug/examples/cpu_temperature_probe.exe。

SHA-256：AE6E0A0FEACCA231B393FF21D9D4CF7A787A5C8C4979722A821D27A983C07871。

程序不安装、不自动提升、不启动主界面或写应用历史。协调进程管理自己的 Job Object 和限定 worker，结束时关闭自身资源；生成一个新的随机命名临时 JSON 报告，不覆盖现有文件。仅一项针对性单位检查验证温度比例、范围/Tj 选择标志和无效零/全一寄存器；严格 Clippy 和 Windows 构建已通过，没有重跑全套测试。

安装授权后，先核对真实安装清单、驱动散列/版本与权限，再执行上述单次管理员读取。成功取得有效温度后才推进 CPU 页面、历史和按需采集组件；读取失败不算本机温度验收通过。详细可审阅数据见 [组件清单](windows-cpu-temperature-component-manifest.json)、[普通权限结果](windows-cpu-temperature-normal-evidence.json)。

## 授权安装与单次读取结果（2026-09-30）

用户明确允许安装和一次限时验证后，使用上述已核验安装器执行 -install -silent，返回 0，未要求重启。没有选用 unrestricted/debug 版本或改动安全策略。安装后确认 PawnIO 服务为内核驱动、按需启动且处于 Running；驱动位于 Windows DriverStore，散列与批准的 AMD64 正式版一致，catalog 签名有效。

实际应用目录为 C:\Program Files\PawnIO，包含 PawnIOLib.dll、PawnIOLib.h、PawnIOUtil.exe 和 uninstall.exe；卸载入口为“已安装的应用”中的 PawnIO，或该卸载器的 -uninstall 模式。该系统组件保留，不因本软件或验证程序退出而卸载。

仅执行一次已准备的管理员程序。三个观测均返回 ok、错误码为空，Tctl 控制温度分别为 67.625°C、68.25°C、67.5°C。协调程序返回 0，已确认协调/读取进程均退出；没有再次提升权限或写入应用温度历史。此前“尚未安装/未取得温度”是准备阶段的记录，由本节替代。详见 [安装与读取证据](windows-cpu-temperature-admin-evidence.json)。

本次确认当前 HX 370 可以通过该签名模块取得 Tctl，未进行独立传感器校准。CPU 页面、历史记录和按需辅助进程仍待接入，不能把此次来源验证称为整个 CPU 功能交付。
