# Mac 探测报告契约（v0.1）

日期：2026-09-11。状态：定义中，未执行探测。

本文件定义后续 M0 探测输出的最小字段和脱敏规则，确保结果可被能力矩阵和验收清单直接引用。

## 1. 顶层结构

每份报告为 JSON，字段如下：

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `report_version` | string | 固定为 `"0.1"` |
| `generated_at` | string (RFC3339) | 报告生成时间 |
| `host` | object | 系统与芯片信息，见 §2 |
| `permissions` | object | 当前执行权限与增强状态，见 §3 |
| `probes` | array | 各指标探测结果，见 §4 |
| `summary` | object | 成功/失败/未知计数，以及阻塞项列表 |

## 2. `host`

| 字段 | 说明 |
| --- | --- |
| `model` | 设备型号标识，如 `Mac16,10` |
| `chip` | 芯片名称，如 `Apple M4` |
| `os_version` | macOS 版本，如 `26.5.2` |
| `build` | 系统构建号 |
| `arch` | 架构，如 `arm64` |
| `memory_bytes` | 物理内存总量（字节，十进制） |
| `cpu_physical_cores` | 物理核心数 |
| `cpu_logical_cores` | 逻辑处理器数 |
| `cpu_perf_levels` | 性能等级数组，每项含 `name`、`physical`、`logical` |

不记录：设备序列号、主机名、用户目录路径。

## 3. `permissions`

| 字段 | 说明 |
| --- | --- |
| `mode` | `"normal"` \| `"elevated"` |
| `enhanced_authorized` | boolean，是否已授权增强采集 |
| `enhanced_available` | boolean，增强组件是否可用 |
| `notes` | 附加说明，如“未提权，未安装组件” |

## 4. `probes` 数组项

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `id` | string | 探测项唯一标识，如 `cpu.total_usage` |
| `category` | string | `cpu` \| `gpu` \| `memory` \| `disk` \| `process` \| `thermal` \| `fan` |
| `description` | string | 探测目标 |
| `source` | string | 采集来源，如 `mach_host_processor_info`、`iokit_perfstats` |
| `status` | string | `verified` \| `partial` \| `blocked_permission` \| `unavailable` \| `error` \| `not_implemented` |
| `value` | any | 实际读数或结构；`null` 表示未获得 |
| `unit` | string | 单位，如 `%`、`bytes`、`celsius`、`bytes_per_second` |
| `sampled_at` | string (RFC3339) | 采样时间 |
| `sample_window_ms` | number | 采样窗口（毫秒）；差分指标必填 |
| `confidence` | string | `high` \| `medium` \| `low` |
| `notes` | string | 附加说明，如“首次采样，需差分” |
| `evidence` | object | 原始输出摘要或校验信息，已脱敏 |

## 5. 脱敏规则

- 不记录：序列号、UUID、主机名、用户主目录、文件路径、进程命令行、网络地址。
- 型号、芯片、容量、计数、百分比、枚举名称可记录。
- 进程探测只记录 PID、名称、内存/IO 数值；不记录路径、参数、打开文件。

## 6. 状态语义

- `verified`：已读取到有效值，且来源、单位、窗口明确。
- `partial`：部分字段可读，但整体口径不完整或需增强权限。
- `blocked_permission`：接口存在但当前权限被拒绝。
- `unavailable`：系统/硬件明确不提供该指标。
- `error`：探测执行出错，附错误摘要。
- `not_implemented`：探测逻辑尚未编写，不能误报为不支持。

## 7. 输出位置

- 报告写入 `docs/probes/<timestamp>-<hostname>.json`，`<hostname>` 用脱敏标识如 `mac-m4`。
- 同时更新 `docs/capability-matrix.md` 对应行的“当前证据”列，附报告路径。
EOF

mkdir -p docs/probes
mkdir -p scripts/probes

cat > scripts/probes/README.md <<'EOF'
# Mac 探测脚本

本目录存放 M0 阶段的只读探测脚本。所有脚本必须：

- 以普通用户权限运行，不调用 sudo。
- 不修改系统设置，不安装组件，不写入用户目录以外位置。
- 输出符合 `docs/probes/contract.md` 的 JSON。
- 脱敏处理序列号、路径、进程命令行等敏感信息。
- 采样窗口明确标注，差分指标必须说明两次采样间隔。

脚本命名：`probe-<category>.sh` 或 `probe-<category>.py`，执行入口统一为 `run-all.sh`。
EOF

touch scripts/probes/.gitkeep
