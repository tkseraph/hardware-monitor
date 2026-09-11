# 总览存储设备区 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在总览页展示每块 SSD 的名称、占用率（按容器计一次）与温度（实时值 + 迷你走势）。

**Architecture:** 纯前端改动。占用率由 `diskUsagePercent` 从 `systemInfo.storage` 拓扑派生；新增 `StorageOverview`/`StorageRow` 组件渲染存储列表区；温度走势复用现有 `Chart`，每行经 `get_history("disk.temperature", device, 3600)` 拉取。不改 Rust / IPC / 历史库。

**Tech Stack:** React 19 + TypeScript（`monitor-app/src/App.tsx`）、CSS（`monitor-app/src/styles.css`）、Tauri IPC（`get_system_info` / `get_history`）。

**Spec:** [2026-09-11-overview-storage-design.md](../specs/2026-09-11-overview-storage-design.md)

## Global Constraints

- 缺值不伪造、不用 0 兜底：占用率/温度无数据时显示 `—`；无历史时不渲染走势图（不留空框）。
- 占用率口径：按容器归属计一次 = Σ`capacity_in_use` / Σ`capacity_ceiling`；无容器或 ceiling 空/0 → `null`。
- 名称兜底：`name || device`；介质名（MediaName）为主，设备号（disk0）为副标。
- 温度单位 `°C`；复用 `Chart` 时 `unit="°C"`、`gap_secs={10}`。
- 文案走 `translations`（`useText()`），新增 key：`"Storage Devices"`/`"Usage"`/`"Temp"`。
- 顶部三卡：`.grid` 改 3 列（`repeat(3,minmax(0,1fr))`），窄屏 ≤640px 回退 1 列。
- 存储区为通栏 `.storage-card`；走势图为 `.chart--mini`（高约 64px）无边框变体。
- 不改 Rust、IPC、历史库；不动磁盘详情页。

---

### Task 1: 占用率派生纯函数 `diskUsagePercent`

**Files:**
- Modify: `monitor-app/src/App.tsx`（在 `formatBytes` 附近新增纯函数；在 `PhysicalDisk`/`ContainerInfo` 接口之后）

**Interfaces:**
- Consumes: 现有类型 `PhysicalDisk { containers: ContainerInfo[] }`、`ContainerInfo { capacity_in_use: number|null; capacity_ceiling: number|null }`（见 App.tsx:72-89）。
- Produces: `function diskUsagePercent(disk: PhysicalDisk): number | null` —— 返回 0–100 的占用率百分比；无容器、ceiling 空/0、或 in_use>ceiling（异常）时返回 `null`。供 Task 3 的 `StorageRow` 使用。

- [ ] **Step 1: 实现纯函数**

在 `App.tsx` 的 `formatBytes` 之后新增：

```tsx
// Occupancy attributed per physical disk: sum each APFS container's
// in_use / ceiling once (S5). Returns null when not computable so the
// UI shows "—" instead of fabricating a value.
function diskUsagePercent(disk: PhysicalDisk): number | null {
  if (disk.containers.length === 0) return null;
  let inUse = 0;
  let ceiling = 0;
  for (const c of disk.containers) {
    if (c.capacity_in_use !== null) inUse += c.capacity_in_use;
    if (c.capacity_ceiling !== null) ceiling += c.capacity_ceiling;
  }
  if (ceiling <= 0) return null;
  if (inUse > ceiling) return null; // anomalous reading; don't fabricate
  return (inUse / ceiling) * 100;
}
```

- [ ] **Step 2: 类型检查**

Run: `cd monitor-app && npm run build`
Expected: tsc + vite 构建通过，无类型错误（函数暂未被引用，tsc 默认不报错；若有 unused 警告属正常）。

- [ ] **Step 3: Commit**

```bash
git add monitor-app/src/App.tsx
git commit -m "feat(overview): diskUsagePercent derivation helper"
```

---

### Task 2: 迷你温度走势组件 `TempSparkline`

**Files:**
- Modify: `monitor-app/src/App.tsx`（在 `Chart` 组件之后新增）

**Interfaces:**
- Consumes: 现有 `Chart({ history, label, unit, gap_secs })`（App.tsx:286）；`invoke<[number,number][]>("get_history", { metricId, objectId, durationSecs })`。
- Produces: `function TempSparkline({ device }: { device: string }): JSX.Element | null` —— 拉取该设备最近 1 小时 `disk.temperature` 历史并渲染迷你 `Chart`；无历史或拉取失败时返回 `null`。供 Task 3 的 `StorageRow` 使用。

- [ ] **Step 1: 实现组件**

在 `Chart` 组件定义之后新增：

```tsx
// Per-disk temperature sparkline for the overview storage row. Returns
// null when there is no history (or the fetch fails) so no empty frame
// is rendered — honest absence, never a fabricated zero.
function TempSparkline({ device }: { device: string }) {
  const [history, setHistory] = useState<[number, number][]>([]);

  useEffect(() => {
    let cancelled = false;
    const fetchHistory = async () => {
      try {
        const data = await invoke<[number, number][]>("get_history", {
          metricId: "disk.temperature",
          objectId: device,
          durationSecs: 3600,
        });
        if (!cancelled) setHistory(data);
      } catch (err) {
        console.error("Failed to fetch disk temperature history:", err);
      }
    };
    fetchHistory();
    const interval = setInterval(fetchHistory, 5000);
    return () => {
      cancelled = true;
      clearInterval(interval);
    };
  }, [device]);

  if (history.length === 0) return null;
  return (
    <div className="chart--mini">
      <Chart history={history} label={`temp-${device}`} unit="°C" gap_secs={10} />
    </div>
  );
}
```

注：`Chart` 的渐变 `id` 用 `label` 拼接，故每个设备传唯一 `label`（`temp-${device}`）避免 SVG id 冲突。

- [ ] **Step 2: 类型检查**

Run: `cd monitor-app && npm run build`
Expected: 构建通过，无类型错误。

- [ ] **Step 3: Commit**

```bash
git add monitor-app/src/App.tsx
git commit -m "feat(overview): TempSparkline per-disk temperature trend"
```

---

### Task 3: 存储行 `StorageRow` 与列表区 `StorageOverview`

**Files:**
- Modify: `monitor-app/src/App.tsx`（在 `OverviewPage` 之前新增两个组件）

**Interfaces:**
- Consumes: `diskUsagePercent`（Task 1）、`TempSparkline`（Task 2）、`useText()`、`formatBytes`、现有 `PhysicalDisk`/`storage` 类型、`Icon`。
- Produces:
  - `function StorageRow({ disk }: { disk: PhysicalDisk }): JSX.Element` —— 单行：名称+设备号副标、占用率进度条、实时温度、温度走势。
  - `function StorageOverview({ storage }: { storage: PhysicalDisk[] }): JSX.Element | null` —— 通栏卡，`storage` 为空返回 `null`。供 Task 4 的 `OverviewPage` 使用。

- [ ] **Step 1: 实现组件**

在 `OverviewPage` 之前新增：

```tsx
function StorageRow({ disk }: { disk: PhysicalDisk }) {
  const t = useText();
  const usage = diskUsagePercent(disk);
  return (
    <div className="storage-row">
      <div className="storage-row-head">
        <Icon name="disk" />
        <div className="storage-name">
          <strong>{disk.name || disk.device}</strong>
          <small>{disk.device}</small>
        </div>
        <div className="storage-usage">
          {usage === null ? (
            <span className="storage-na">—</span>
          ) : (
            <>
              <div className="progress-bar">
                <div className="progress" style={{ width: `${usage}%` }}></div>
              </div>
              <span className="storage-pct">{usage.toFixed(0)}%</span>
            </>
          )}
        </div>
        <div className="storage-temp">
          {disk.temperature_celsius === null ? (
            <span className="storage-na">—</span>
          ) : (
            <span className="value">{disk.temperature_celsius.toFixed(1)}°C</span>
          )}
        </div>
      </div>
      <TempSparkline device={disk.device} />
    </div>
  );
}

function StorageOverview({ storage }: { storage: PhysicalDisk[] }) {
  const t = useText();
  if (storage.length === 0) return null;
  return (
    <div className="card storage-card">
      <div className="card-header">
        <Icon name="disk" />
        <div>
          <h3>{t("Storage Devices")}</h3>
          <small>{storage.length} {t("Disks")}</small>
        </div>
      </div>
      <div className="storage-rows">
        {storage.map((d) => (
          <StorageRow key={d.device} disk={d} />
        ))}
      </div>
    </div>
  );
}
```

- [ ] **Step 2: 补翻译**

在 `translations`（App.tsx:6）中追加 key（注意该行是单行大对象，追加在末尾 `}`.前，保持 JSON 逗号正确）：

`"Storage Devices": "存储设备", "Usage": "占用率", "Temp": "温度"`

- [ ] **Step 3: 类型检查**

Run: `cd monitor-app && npm run build`
Expected: 构建通过（组件暂未被 `OverviewPage` 引用）。

- [ ] **Step 4: Commit**

```bash
git add monitor-app/src/App.tsx
git commit -m "feat(overview): StorageOverview and StorageRow components"
```

---

### Task 4: 接入总览页（移除磁盘大卡，改为 3 卡 + 存储区）

**Files:**
- Modify: `monitor-app/src/App.tsx`（`OverviewPage`，App.tsx:197-275）

**Interfaces:**
- Consumes: `StorageOverview`（Task 3）、`systemInfo.storage`。
- Produces: 更新后的 `OverviewPage`；总览 = CPU/内存/GPU 三卡 + 通栏存储区。

- [ ] **Step 1: 替换 OverviewPage 布局**

将 `OverviewPage` 的 return 替换为（移除磁盘 hero 卡，GPU 卡后闭合 `.grid`，再挂 `StorageOverview`）：

```tsx
function OverviewPage({ systemInfo }: { systemInfo: SystemInfo }) {
  const t = useText();
  return (
    <div>
      <h2>{t("System Overview")}</h2>
      <div className="grid">
        <div className="card hero-card">
          <div className="card-header">
            <Icon name="cpu" />
            <div>
              <h3>{t("CPU")}</h3>
              <small>{systemInfo.cpu.name}</small>
            </div>
          </div>
          <div className="big-value">{systemInfo.cpu.total_usage.toFixed(1)}%</div>
          <div className="progress-bar">
            <div className="progress" style={{ width: `${systemInfo.cpu.total_usage}%` }}></div>
          </div>
          <div className="info-row">
            <span>{t("Cores:")} {systemInfo.cpu.physical_cores} / {systemInfo.cpu.logical_cores}</span>
          </div>
        </div>

        <div className="card hero-card">
          <div className="card-header">
            <Icon name="memory" />
            <div>
              <h3>{t("Memory")}</h3>
              <small>{formatBytes(systemInfo.memory.total_bytes)}</small>
            </div>
          </div>
          <div className="big-value">{systemInfo.memory.used_percent.toFixed(1)}%</div>
          <div className="progress-bar">
            <div className="progress" style={{ width: `${systemInfo.memory.used_percent}%` }}></div>
          </div>
          <div className="info-row">
            <span>{t("Used:")} {formatBytes(systemInfo.memory.used_bytes)}</span>
          </div>
        </div>

        <div className="card hero-card">
          <div className="card-header">
            <Icon name="gpu" />
            <div>
              <h3>{t("GPU")}</h3>
              <small>{systemInfo.gpu.name}</small>
            </div>
          </div>
          <div className="big-value">{systemInfo.gpu.utilization.toFixed(1)}%</div>
          <div className="progress-bar">
            <div className="progress" style={{ width: `${systemInfo.gpu.utilization}%` }}></div>
          </div>
          <div className="info-row">
            <span>{t("Memory:")} {formatBytes(systemInfo.gpu.memory_used_bytes)}</span>
          </div>
        </div>
      </div>

      <StorageOverview storage={systemInfo.storage} />
    </div>
  );
}
```

注：原磁盘 hero 卡用到 `systemInfo.disks.length` 与 `disk_throughput`，移除后总览不再用这两个字段做「Active I/O」；磁盘详情页仍用，不动。

- [ ] **Step 2: 类型检查**

Run: `cd monitor-app && npm run build`
Expected: 构建通过。

- [ ] **Step 3: Commit**

```bash
git add monitor-app/src/App.tsx
git commit -m "feat(overview): 3 top cards + storage device section"
```

---

### Task 5: 样式（3 列网格、存储区、行、迷你走势）

**Files:**
- Modify: `monitor-app/src/styles.css`（`.grid` 改 3 列；追加存储区样式；窄屏媒体查询回退）

**Interfaces:**
- Consumes: Task 3/4 的 class 名：`storage-card`、`storage-rows`、`storage-row`、`storage-row-head`、`storage-name`、`storage-usage`、`storage-pct`、`storage-temp`、`storage-na`、`chart--mini`。
- Produces: 上述类的样式；`.grid` 3 列；`chart--mini` 高度约 64px。

- [ ] **Step 1: 改 `.grid` 为 3 列**

`styles.css:1` 中 `.grid{...grid-template-columns:repeat(2,minmax(0,1fr));...}` 改为 `repeat(3,minmax(0,1fr))`。

- [ ] **Step 2: 追加存储区样式**

在 `styles.css` 主规则（line 1）的 `.chart-labels{...}` 之后追加（仍在同一行内）：

```css
.storage-card{margin-top:18px}
.storage-rows{display:flex;flex-direction:column}
.storage-row{padding:14px 0;border-bottom:1px solid rgba(0,0,0,.06)}
.storage-row:last-child{border-bottom:0}
.storage-row-head{display:flex;align-items:center;gap:14px}
.storage-row-head>svg{width:22px;height:22px;color:var(--accent);flex-shrink:0}
.storage-name{display:flex;flex-direction:column;min-width:0;flex:1}
.storage-name strong{font-size:13px;font-weight:600;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.storage-name small{color:var(--muted);font-size:11px}
.storage-usage{display:flex;align-items:center;gap:10px;min-width:180px}
.storage-usage .progress-bar{flex:1;margin-top:0}
.storage-pct{font-variant-numeric:tabular-nums;font-weight:600;font-size:13px;min-width:38px;text-align:right}
.storage-temp{min-width:70px;text-align:right}
.storage-temp .value{font-variant-numeric:tabular-nums;font-weight:600}
.storage-na{color:#94a3b8}
.chart--mini{margin-top:10px}
.chart--mini .chart{margin-top:0;padding:0;border:0;background:transparent}
.chart--mini .chart svg{height:64px}
.chart--mini .chart-labels{display:none}
```

- [ ] **Step 3: 窄屏回退**

`styles.css:4` 媒体查询中 `.grid{grid-template-columns:1fr}` 已存在（覆盖 3 列），确认保留；另在该媒体查询内追加：

```css
.storage-usage{min-width:120px}
.storage-row-head{flex-wrap:wrap}
```

- [ ] **Step 4: 构建验证**

Run: `cd monitor-app && npm run build`
Expected: 构建通过（CSS 无类型检查，仅确认打包成功）。

- [ ] **Step 5: Commit**

```bash
git add monitor-app/src/styles.css
git commit -m "style(overview): 3-col grid and storage device rows"
```

---

### Task 6: 浏览器验证 + release 构建

**Files:**
- 无新增；验证既有改动。

- [ ] **Step 1: dev server 渲染核对**

起 dev server，浏览器打开总览，用 `preview_snapshot`/`preview_inspect` 核对：存储区显示两块 SSD（disk0/disk4）的名称、占用率进度条与百分比、实时温度、迷你温度走势；缺值显示 `—`；顶部为三卡。

- [ ] **Step 2: 截图**

`preview_screenshot` 截取总览，确认布局与真实数据。

- [ ] **Step 3: release 构建**

Run: `cd monitor-app && bash ../scripts/build-release.sh 2>&1 | tail -20`
Expected: 构建 + 签名 + DMG 校验通过，输出新 SHA-256。

- [ ] **Step 4: Commit（如有构建产物文档更新）**

```bash
git add -A
git commit -m "chore(release): rebuild with overview storage section"
```

---

## Self-Review

- **Spec coverage**：名称（Task 3 `name||device`+副标）、占用率按容器计一次（Task 1）、实时温度+迷你走势（Task 2/3）、缺值不伪造（Task 1 返回 null / Task 3 显示 `—` / Task 2 返回 null）、3 列网格+通栏存储区（Task 4/5）、翻译 key（Task 3 Step 2）、复用 Chart `unit="°C"`、`gap_secs={10}`（Task 2）—— 全覆盖。
- **Placeholder scan**：无 TBD/TODO；所有代码步骤含完整代码。
- **Type consistency**：`diskUsagePercent(disk: PhysicalDisk): number | null`（Task 1）= Task 3 调用签名；`TempSparkline({ device }: { device: string })`（Task 2）= Task 3 `<TempSparkline device={disk.device} />`；`StorageOverview({ storage }: { storage: PhysicalDisk[] })`（Task 3）= Task 4 `<StorageOverview storage={systemInfo.storage} />`。一致。
