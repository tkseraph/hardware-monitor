# 总览存储设备区设计（SSD 名称 / 占用率 / 温度）

- 日期：2026-09-11
- 状态：已获用户批准
- 范围：仅在总览页展示固态设备的名称、占用率（磁盘使用量）与温度；纯前端改动，不动 Rust 采集链、IPC、历史库。

## 1. 背景与目标

当前总览为 2×2 大卡片（CPU / 内存 / GPU / 磁盘），其中磁盘卡只显示「有吞吐的设备数」，缺少每块 SSD 的名称、占用率、温度。用户希望在总览直接看到这三项。

后端数据已具备：
- `SystemInfo.storage: PhysicalDisk[]`（S5 拓扑）：每盘含 `device`（设备号）、`name`（MediaName 介质名）、`temperature_celsius`（SMART 温度）、`containers`（APFS 容器，含 `capacity_in_use` / `capacity_ceiling`）。
- 历史库已有 `disk.temperature` 每设备时序（S9 写入），`get_history("disk.temperature", device, durationSecs)` 可查。

因此占用率只是现有拓扑的派生展示，温度走势复用现有图表与历史接口，无需改后端。

## 2. 关键决策（用户已选）

| 决策点 | 结论 |
| --- | --- |
| 展示形式 | 独立的「存储设备」列表区，每块 SSD 一行（非大卡片内嵌、非摘要行） |
| 占用率口径 | 按容器归属计一次：占用率 = Σ容器 capacity_in_use / Σ容器 capacity_ceiling，符合 S5「容量按容器计一次」 |
| 温度展示 | 行内实时温度 + 最近一小时迷你走势图 |
| 名称显示 | 介质名（MediaName）为主，设备号（disk0）为副标；介质名为空时兜底显示设备号 |
| 实现方案 | 方案 A：前端派生。不改 Rust / IPC / 历史库 |

## 3. 架构与数据流

```
后端 SystemInfo（已具备，无需改）
├─ storage: PhysicalDisk[]  → 名称(device/name)、温度(temperature_celsius)、容器容量
└─ 历史库 disk.temperature   → 迷你走势时序（S9 已写入）

前端总览
├─ 派生每盘占用率：diskUsagePercent(disk) 纯函数（按容器计一次）
└─ 新增组件 StorageOverview / StorageRow：拉温度历史 + 渲染存储列表区
```

- 实时数据来源：`systemInfo.storage`。
- 走势数据来源：每行 `get_history("disk.temperature", device, 3600)`，5s 轮询（与详情页一致）。

## 4. 组件与布局

总览结构：CPU / 内存 / GPU 保留为 3 张大卡片；移除原「磁盘」大卡，下方新增一个通栏「存储设备」卡，内部每块 SSD 一行。

新增前端组件（均在 `App.tsx` 内，沿用现有模式）：
- `StorageOverview({ storage })`：通栏卡，map 每盘渲染一个 `StorageRow`。
- `StorageRow({ disk })`：单盘行——名称（`name || device`）+ 设备号副标 + 占用率进度条 + 实时温度 + 迷你走势。
- `diskUsagePercent(disk): number | null`：占用率派生纯函数。
- 温度走势：复用 `<Chart unit="°C" gap_secs={10}>`，缩小为行内迷你变体。

每行字段优先级：名称（介质名，空则设备号）> 占用率（无容器则 `—`）> 温度（空则 `—`）> 走势（无历史则不渲染图，不留空白框）。

布局示意：

```
┌─────────────┬─────────────┬─────────────┐
│  CPU 大卡   │  内存大卡   │  GPU 大卡   │   ← 3 列
├─────────────┴─────────────┴─────────────┤
│  存储设备（通栏卡）                       │
│   💽 APPLE SSD AP0512Z [disk0]           │
│      占用率 ▓▓▓▓▓▓░░ 62%   温度 41.2°C   │
│      [温度迷你走势]                       │
│   💽 APPLE SSD AP1024Z [disk4]           │
│      占用率 ▓▓▓░░░░░ 35%   温度 38.7°C   │
│      [温度迷你走势]                       │
└──────────────────────────────────────────┘
```

## 5. 样式

追加到 `styles.css`，沿用现有设计令牌，不引入新色系：
- 顶部三卡：`.grid` 由 2 列改 3 列（`repeat(3,minmax(0,1fr))`），窄屏（≤640px）仍回退 1 列。
- 存储区：独立 `.storage-card` 通栏。
- 每行 `.storage-row`：flex 行；名称+设备号左对齐，占用率进度条（复用 `.progress-bar`/`.progress`）+ 百分比居中，温度右对齐（`.value` 数字风格，`tabular-nums`）。
- 迷你走势：`.chart--mini` 变体，高度约 64px，无边框融入行内；行间分隔复用 `.disk-info` 的 `border-bottom` 风格。
- 设备号副标用 `.card-header small` 的 muted 样式。

文案（补 `translations`）：`"Storage Devices": "存储设备"`、`"Usage": "占用率"`、`"Temp": "温度"` 等；设备号副标与 `°C` 无需翻译。

## 6. 缺值与错误处理（不伪造、不用 0 兜底）

- 占用率：该盘无容器，或 `capacity_ceiling` 为空/为 0 → 显示 `—`，不渲染进度条。
- 温度：`temperature_celsius` 为 `null` → 显示 `—`，不显示 `0°C`。
- 温度走势：`get_history` 返回空数组 → 该行不渲染走势图（`Chart` 在 `history.length===0` 时返回 `null`）。
- 历史拉取失败：`catch` 仅 `console.error`，走势区留空，不影响名称/占用率/实时温度。
- 无任何存储设备（`storage` 为空）：整个存储区不渲染。

## 7. 测试与验收

- 纯函数 `diskUsagePercent`：无依赖、可单测，覆盖单容器、多容器求和、无容器、ceiling=0、in_use>ceiling（异常）等边界。前端若无测试框架，则以 `npm run build`（tsc 类型检查）+ 人工核对为主。
- `StorageOverview` 仅依赖 `systemInfo.storage`；后端 `build_topology()` 失败时该字段为 `unwrap_or_default()`（空数组），前端天然兜底为不渲染存储区。
- 验收点：总览存储区显示两块 SSD（disk0 / disk4）的名称、占用率进度条、实时温度、迷你温度走势；缺值场景显示 `—`。
- 验证方式：改完起 dev server 截图核对布局与真实数据，再构建 release 确认。

## 8. 非目标（YAGNI）

- 不把占用率写入历史库、不提供占用率走势（当前仅需温度走势）。
- 不改 CPU/GPU 核心温度（保持未实现，不自动提权）。
- 不动磁盘详情页既有功能（吞吐、SMART、通电时间等）。
