# 本机硬件监控软件

Hardware Monitor 使用 Tauri 2、React/TypeScript、Rust 和 SQLite，提供本机硬件实时监控与设备级历史。

Windows 适配正在实施。当前 Windows x64 开发版本已具备 CPU 总/逐逻辑处理器、系统内存和进程内存/CPU 的真实读数；采用独立来源线程与有界历史写入队列。本机两块 NVMe 的盘卷关系、容量、实时吞吐和匿名历史已接入；Radeon 890M 的 GPU 全局利用率和专用/共享内存已接入；进程存储 I/O 与增强传感器仍未完成，界面明确显示其状态。可编译或可运行不代表完整基础版本验收通过。

现有 Mac 实现保留，历史验收与待验证项见 [Mac 整改记录](docs/remediation-acceptance.md) 和 [R14 记录](docs/remediation-acceptance-r14.md)。Windows 本轮修改的 Mac 原生回归尚未执行。

## Windows 开发入口

- [分步实施计划](docs/plans/2026-09-14-windows-development-plan.md)：W0～W7 的依赖、工作项和验收条件。
- [开发环境](docs/windows-environment.md)：已核实的工具链与本机条件。
- [来源能力](docs/windows-capabilities.md)：探针可读与生产集成分别记录。
- [数据契约](docs/windows-metric-contract.md)：平台、缺值、来源状态、历史和进程身份。
- [Windows 验收记录](docs/windows-acceptance.md)：实际测试、原生验证和剩余项目。

开发工具版本以 `.node-version`、`rust-toolchain.toml` 和两个依赖锁文件为准。Windows 使用 MSVC、Windows SDK 与 WebView2。测试时设置独立的 `MONITOR_DATA_DIR`，Windows 的数据库、设置、WebView 和 debug 日志均位于对应测试根。构建产物与真实测试数据不入库。

## 产品范围

总览、CPU/GPU/内存/磁盘详情、进程排行及设置；存储以物理盘和关联卷组织；设备级历史本地保留 7 天，不默认保存进程明细历史。关闭窗口后继续采集，明确退出才停止；主要动态指标目标为前台 1 秒、后台 3 秒，自启默认关闭。

普通权限基础模式与可选增强采集分开。不填造缺失读数，不把尚未实现写成设备不支持，不上传监控数据。Windows 进程结束和自启目前禁用；没有安装监控服务或硬件驱动。

## 需求与既有设计

[需求与决策](docs/requirements.md)、[领域模型](docs/domain-model.md)、[原 Mac 实施方案](docs/implementation-plan.md)、[既有能力矩阵](docs/capability-matrix.md)、[验收原则](docs/validation-plan.md)保留用于溯源。其中 Windows 延后的表述属于旧阶段，当前 Windows 工作以新的实施计划和实际记录为准。
