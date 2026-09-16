## Windows 开发环境

本记录对应 2026-09-14 开始实施的首批 Windows 适配。应用基线为 `6009e750e8fd8571488961da204add13191fde4a`；本轮改动尚未提交，基线 SHA 不代表本轮二进制的完整源码。

| 项目 | 当前证据 |
| --- | --- |
| 系统 | Windows 11 专业版 25H2，x64，26200.9445，普通用户会话 |
| CPU | Ryzen AI 9 HX 370；12 个物理核、24 个逻辑处理器 |
| 系统内存 | 本项目 sysinfo 实测系统可使用总量 59,719,397,376 字节，约 55.62 GiB；不是当前空闲量 |
| Node / npm | 24.19.0 / 11.17.0，前端类型、单元测试和构建通过 |
| Rust / Cargo | 1.98.1，x86_64-pc-windows-msvc；rustfmt、Clippy 可用，项目工具链已固定 |
| C++ | Visual Studio Build Tools 2022，17.14 系列；MSVC 14.44.35207 |
| Windows SDK | 10.0.26100.0 |
| WebView2 | 已有运行时 152.0.4191.66，未重装 |
| 原生构建 | Windows cargo check、严格 Clippy 与 Tauri debug exe 构建通过 |

用户明确授权安装缺失开发组件。Rust 安装器来自官方静态分发站并比对公开 SHA-256，未修改系统 PATH；C++ 安装器经过有效 Microsoft 签名校验，由用户手动安装。初次仅安装 Build Tools 主体，补装 C++ 工作负载和 SDK 后，原生链接器才可用。未安装监控服务、传感器驱动或增强组件。

本机 Rust 命令可在当前 PowerShell 会话加入用户 Cargo bin 后执行，不需要修改全局环境：

```powershell
$env:PATH = "$env:USERPROFILE\.cargo\bin;" + $env:PATH
```

原生验收使用独立临时数据根；Windows WebView 用户数据和 debug 日志已与后端根一起隔离。没有读取或迁移 Mac 历史库。测试目录保留在仓库外，不作为源码提交。

参考：[Tauri Windows 前置条件](https://v2.tauri.app/start/prerequisites/)。
