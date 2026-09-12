import { orderOverviewDisks } from "./storage-order";
import { useTheme } from "./theme-hook";
import type { ThemePreference } from "./theme";
import { TemperatureTrend } from "./TemperatureTrend";
import { storageUsage } from "./storage-usage";
import { useHistoryQuery, downsamplePreserveExtremes, gapThresholdSecs } from "./history-hooks";
import { createContext, useContext, useEffect, useRef, useState } from "react";
import { invoke, isTauri } from "@tauri-apps/api/core";

type Language = "zh" | "en";
const LanguageContext = createContext<Language>("zh");
const translations: Record<string, string> = {"System Overview": "系统总览", "CPU Details": "处理器详情", "Memory Details": "内存详情", "GPU Details": "图形处理器详情", "Disk Details": "磁盘详情", "Process Ranking": "进程排行", "Settings": "设置", "No matching processes": "暂无匹配的进程", "General": "通用", "CPU": "处理器", "GPU": "图形处理器", "Memory": "内存", "Disks": "存储设备", "Name:": "名称", "Cores:": "核心数量", "Usage:": "使用率", "Total:": "总容量", "Used:": "已使用", "Utilization:": "利用率", "Memory:": "内存用量", "Physical Cores:": "物理核心", "Logical Processors:": "逻辑处理器", "Total Usage:": "总使用率", "Per-Core Usage": "逐核使用率", "Usage History (Last Hour)": "使用率历史 · 最近一小时", "System Memory": "系统内存", "Available:": "可用", "In Use:": "使用中", "Allocated:": "已分配", "Unified memory architecture - no separate VRAM": "统一内存架构，无独立显存；以下为驱动统计，不代表独立显存容量。", "Device:": "设备标识", "Capacity:": "容量", "SMART Status:": "SMART 摘要", "Temperature:": "温度", "Power On Hours:": "通电时间", "hours": "小时", "Throughput:": "合计吞吐", "Throughput History (Last Hour)": "吞吐历史 · 最近一小时", "Name": "进程名称", "Sort by Memory": "按内存排序", "Sort by CPU": "按 CPU 排序", "Sort by Read": "按读取排序", "Sort by Write": "按写入排序", "Showing": "显示", "of": "共", "readable processes (system-wide disk I/O)": "个可读取进程（磁盘读写为系统范围）", "Failed to load processes": "进程加载失败", "Read/s": "读取/秒", "Write/s": "写入/秒", "Settings will be implemented in a future update.": "采样频率、历史保留与登录项设置尚未实现。", "Sampling": "采样", "Foreground interval (ms)": "前台采样间隔（毫秒）", "Background interval (ms)": "后台采样间隔（毫秒）", "Startup": "启动", "Launch at login": "登录时启动", "On": "开", "Off": "关", "Closing the window keeps monitoring in the menu bar; Quit stops collection.": "关闭窗口后在菜单栏继续采集；选择退出才停止。", "Settings saved": "设置已保存", "Failed to save settings": "设置保存失败", "Settings are available in the desktop app": "设置仅在桌面应用中可用", "Loading settings…": "正在加载设置…", "Storage Devices": "存储设备", "Usage": "占用率", "Temp": "温度", "Unreadable": "不可读", "matching": "个匹配", "Prev": "上一页", "Next": "下一页", "Page": "第", "page": "条/页", "Rows per page": "每页行数"};
Object.assign(translations, {"Physical capacity": "总容量（物理盘）", "APFS capacity basis": "占用率口径：APFS 容器容量", "End process": "结束进程", "Select a process": "选择进程", "Cancel": "取消", "Confirm termination": "确认结束", "Requesting…": "正在请求…", "Unsaved work may be lost. Send SIGTERM without force or elevation?": "可能丢失未保存的内容。是否发送普通终止请求（SIGTERM），不强制、不提权？", "Termination requested; process may still be running.": "已发送终止请求；进程可能仍在运行，请查看刷新后的列表。", "This process is protected.": "此进程受保护，不能结束。", "Process already exited.": "进程已退出。", "Process identity changed. Select it again.": "进程身份已变化，请重新选择。", "Permission denied; only your own processes can be ended.": "权限不足；仅允许结束当前用户的进程。", "Failed to request termination.": "发送终止请求失败。", "Selection left the current list; select it again.": "所选进程已不在当前列表中，请重新选择。", "Failed to load settings": "设置加载失败", "Settings file was invalid; defaults restored. The original file was kept.": "设置文件无效；已恢复默认值，原文件已保留。", "Could not verify login item state; left unchanged.": "无法核实登录项状态，已保持原状。", "Login item changed but settings were not saved.": "登录项已更改，但设置未保存。", "Failed to load history": "历史加载失败", "No history yet": "暂无历史数据", "History range": "历史范围", "Throughput History": "吞吐历史"});
function useText() { const lang = useContext(LanguageContext); return (text: string) => lang === "zh" ? translations[text] ?? text : text; }

interface CpuInfo {
  name: string;
  physical_cores: number;
  logical_cores: number;
  total_usage: number;
  per_core_usage: number[];
}

interface MemoryInfo {
  total_bytes: number;
  used_bytes: number;
  available_bytes: number;
  used_percent: number;
}

interface GpuInfo {
  name: string;
  utilization: number;
  memory_used_bytes: number;
  memory_allocated_bytes: number;
}

interface DiskInfo {
  device: string;
  /** Stable anonymous history identity (A10). */
  device_uid: string;
  generation: number;
  name: string;
  size_bytes: number;
  smart_status: string;
  temperature_celsius: number | null;
  power_on_hours: number | null;
}

interface DiskThroughput {
  device: string;
  /** Stable anonymous history identity (A10). */
  device_uid: string;
  mb_per_sec: number;
}

interface ProcessInfo {
  pid: number;
  start_marker: number;
  name: string;
  memory_bytes: number;
  cpu_usage: number;
  disk_read_bytes: number;
  disk_write_bytes: number;
  disk_read_bps: number | null;
  disk_write_bps: number | null;
  /** R6: false when I/O counters were unreadable; do not render the 0s. */
  io_ok: boolean;
}

interface ProcessPage {
  processes: ProcessInfo[];
  total_readable: number;
  /** R6: rows matching the filter before pagination — real page count. */
  matched_total: number;
  offset: number;
  limit: number;
  observed_at: number;
}

type ProcessSortKey = "memory" | "cpu" | "diskread" | "diskwrite";

interface VolumeInfo {
  id: string;
  name: string;
  role: string;
  /** All APFS roles (multi-role volumes are not collapsed to one string). */
  roles: string[];
  capacity_consumed: number | null;
}

interface ContainerInfo {
  container_ref: string;
  /** R7/A09: EVERY backing physical store, never truncated to the first. */
  physical_stores: string[];
  /** True when capacity is shared across more than one physical disk. */
  shared_pool: boolean;
  capacity_ceiling: number | null;
  capacity_free: number | null;
  capacity_in_use: number | null;
  volumes: VolumeInfo[];
}

interface PhysicalDisk {
  /** Volatile enumeration address (e.g. disk0); live display only (A10). */
  device: string;
  /** Stable anonymous history identity (A10); falls back to device if absent. */
  device_uid: string | null;
  /** Generation of device_uid; bumps on hot-plug address reuse (A10). */
  generation: number;
  name: string;
  size_bytes: number;
  smart_status: string;
  temperature_celsius: number | null;
  power_on_hours: number | null;
  containers: ContainerInfo[];
}

interface SystemInfo {
  cpu: CpuInfo;
  memory: MemoryInfo;
  gpu: GpuInfo;
  disks: DiskInfo[];
  disk_throughput: DiskThroughput[];
  /** Full physical-disk → container → volume topology. */
  storage: PhysicalDisk[];
  /** Unix seconds when the snapshot was sampled by the Rust scheduler. */
  observed_at: number;
}

/** R4: sampling + history health from get_system_status. */
interface SamplingHealth {
  last_success_at: number | null;
  success_age_secs: number | null;
  consecutive_failures: number;
  ever_succeeded: boolean;
}
interface SystemStatus {
  sampling: SamplingHealth;
  history_health: "ok" | "over_budget" | "write_error" | "unavailable";
}

type Page = "overview" | "cpu" | "memory" | "gpu" | "disk" | "processes" | "settings";

type IconName = "overview" | "cpu" | "memory" | "gpu" | "disk" | "processes" | "settings";

const iconPaths: Record<IconName, string> = {
  overview: "M4 13h6V4H4v9zm0 7h6v-5H4v5zm10 0h6v-9h-6v9zM4 20h6v-9H4v9zm10-16v5h6V4h-6z",
  cpu: "M8 8h8v8H8zM4 10h2v4H4zm14 0h2v4h-2zM10 4h4v2h-4zm0 14h4v2h-4z",
  memory: "M4 6h16v8H4zM6 10h2v2H6zm4 0h2v2h-2zm4 0h2v2h-2z",
  gpu: "M4 7h16v9H4zM7 10h4v3H7zm6 0h4v3h-4zM6 20h12v-2H6z",
  disk: "M5 4h14l3 7v7H2v-7l3-7zm2 9a2 2 0 1 0 0 4 2 2 0 0 0 0-4zm10 0a2 2 0 1 0 0 4 2 2 0 0 0 0-4z",
  processes: "M4 6h16v2H4zm0 5h16v2H4zm0 5h16v2H4z",
  settings: "M12 8a4 4 0 1 0 0 8 4 4 0 0 0 0-8zm8.9 4.5-.1 1.5 2.1 1.6-2 3.4-2.5-1a7.6 7.6 0 0 1-1.3.8l-.4 2.7h-4l-.4-2.7c-.5-.2-.9-.5-1.3-.8l-2.5 1-2-3.4 2.1-1.6a7.4 7.4 0 0 1 0-1.6L4.6 9.9l2-3.4 2.5 1c.4-.3.8-.6 1.3-.8L10.8 4h4l.4 2.7c.5.2.9.5 1.3.8l2.5-1 2 3.4-2.1 1.6c.1.5.1 1 .1 1.5z",
};

function Icon({ name }: { name: IconName }) {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true" focusable="false">
      <path d={iconPaths[name]} fill="currentColor" />
    </svg>
  );
}

function formatBytes(bytes: number): string {
  if (bytes === 0) return "0 B";
  const k = 1024;
  const sizes = ["B", "KiB", "MiB", "GiB", "TiB"];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return parseFloat((bytes / Math.pow(k, i)).toFixed(2)) + " " + sizes[i];
}

// Occupancy attributed per physical disk: sum each APFS container's
// in_use / ceiling once (S5). Returns null when not computable so the
// UI shows "—" instead of fabricating a value.
export function diskUsagePercent(disk: PhysicalDisk): number | null {
  return storageUsage(disk.containers)?.percent ?? null;
}

function App() {
  const [theme, setTheme] = useTheme();
  const [language, setLanguage] = useState<Language>(() => localStorage.getItem("monitor-language") === "en" ? "en" : "zh");
  const zh = language === "zh";
  useEffect(() => { localStorage.setItem("monitor-language", language); document.documentElement.lang = zh ? "zh-CN" : "en"; }, [language, zh]);
  const [systemInfo, setSystemInfo] = useState<SystemInfo | null>(null);
  const [status, setStatus] = useState<SystemStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [currentPage, setCurrentPage] = useState<Page>("overview");

  useEffect(() => {
    if (!isTauri()) return;
    let busy = false;
    const fetchData = async () => {
      if (busy) return;
      busy = true;
      try {
        const [info, st] = await Promise.all([
          invoke<SystemInfo>("get_system_info"),
          invoke<SystemStatus>("get_system_status"),
        ]);
        setSystemInfo(info);
        setStatus(st);
        setError(null);
      } catch (err) {
        setError(String(err));
      } finally {
        busy = false;
      }
    };

    fetchData();
    const interval = setInterval(fetchData, 1000);
    return () => clearInterval(interval);
  }, []);

  // R4/A03: a cached snapshot is only "realtime" while it is fresh. Foreground
  // cadence is ~1s (user-configurable up to 10s), so a snapshot older than 15s
  // means collection has stalled — never show a stale value as live.
  const FRESH_THRESHOLD_SECS = 15;
  const snapshotAge = status?.sampling.success_age_secs ?? null;
  const isStale = status !== null
    && status.sampling.ever_succeeded
    && snapshotAge !== null
    && snapshotAge > FRESH_THRESHOLD_SECS;
  const historyDegraded = status !== null && status.history_health !== "ok";

  const pages: [Page, string, string, IconName][] = [
    ["overview", "总览", "Overview", "overview"], ["cpu", "处理器", "CPU", "cpu"],
    ["memory", "内存", "Memory", "memory"], ["gpu", "图形处理器", "GPU", "gpu"],
    ["disk", "磁盘", "Storage", "disk"], ["processes", "进程排行", "Processes", "processes"],
    ["settings", "设置", "Settings", "settings"]
  ];
  return (
    <LanguageContext.Provider value={language}>
    <div className="app">
      <nav className="sidebar" aria-label={zh ? "主导航" : "Navigation"}>
        <div className="brand"><span className="brand-icon">M</span><div><h1>Monitor</h1><small>{zh ? "硬件监控中心" : "HARDWARE INSIGHTS"}</small></div></div>
        <div className="nav-caption">{zh ? "工作空间" : "WORKSPACE"}</div>
        <ul>{pages.map(([id, cn, en, icon]) => <li key={id}><button aria-current={currentPage === id ? "page" : undefined} className={currentPage === id ? "active" : ""} onClick={() => setCurrentPage(id)}><Icon name={icon} />{zh ? cn : en}</button></li>)}</ul>
        <div className="sidebar-footer"><span className="privacy-dot" />{zh ? "本地采集 · 隐私优先" : "Local & private"}<small>MONITOR / macOS</small></div>
      </nav>
      <main className="content">
        <header className="topbar"><span>Monitor <span className="crumb">/ {pages.find(p => p[0] === currentPage)?.[zh ? 1 : 2]}</span></span><div className="topbar-controls"><label className="theme-control"><span>{zh ? "外观" : "Appearance"}</span><select aria-label={zh ? "外观模式" : "Appearance mode"} value={theme} onChange={e => setTheme(e.target.value as ThemePreference)}><option value="system">{zh ? "跟随系统" : "System"}</option><option value="light">{zh ? "浅色" : "Light"}</option><option value="dark">{zh ? "深色" : "Dark"}</option></select></label><select aria-label={zh ? "界面语言" : "Language"} value={language} onChange={e => setLanguage(e.target.value as Language)}><option value="zh">简体中文</option><option value="en">English</option></select></div></header>
        <section className="page-heading"><div><div className="eyebrow">HARDWARE MONITOR</div><h2>{zh ? "洞悉设备的每一刻" : "Your hardware, at a glance"}</h2><p>{zh ? "专注关键指标，让系统状态清晰可见。" : "A clear view of the metrics that matter."}</p></div><span className="status-pill">{!isTauri() ? (zh ? "浏览器预览" : "Browser preview") : error ? (zh ? "采集异常" : "Collection error") : isStale ? (zh ? "数据可能过期" : "Data may be stale") : systemInfo ? (zh ? "实时采集中" : "Collecting") : (zh ? "等待采样" : "Waiting")}</span></section>
        {isStale && !error && <div className="note" role="status">{zh ? `采集器已 ${snapshotAge} 秒未更新，显示的为最近成功读数。` : `Collector has not updated for ${snapshotAge}s; showing the last good reading.`}</div>}
        {historyDegraded && !error && <div className="note" role="status">{zh ? "历史记录暂不可用或已暂停写入；实时读数不受影响。" : "History is unavailable or paused; realtime readings are unaffected."}</div>}
        {error && <div className="note" role="alert">{zh ? "采集失败，显示的旧读数可能已过期：" : "Collection failed; previous values may be stale: "}{error}</div>}
        {!systemInfo && currentPage !== "settings" && <section className="empty-panel"><div className="empty-icon"><Icon name="overview" /></div><h3>{zh ? (isTauri() ? "正在连接本机采集器" : "在桌面应用中查看实时数据") : (isTauri() ? "Connecting to collectors" : "Live metrics need the desktop app")}</h3><p>{zh ? "硬件指标由 macOS 原生接口提供。未连接采集器时，不显示模拟读数。" : "Metrics come from native macOS APIs. No simulated readings are displayed."}</p><div className="empty-grid">{pages.slice(1,5).map(([id,cn,en,icon]) => <button key={id} onClick={() => setCurrentPage(id)}><span><Icon name={icon} /></span><strong>{zh ? cn : en}</strong><b>—</b><small>{zh ? "等待真实数据" : "Awaiting real data"}</small></button>)}</div></section>}

        {currentPage === "overview" && systemInfo && <OverviewPage systemInfo={systemInfo} />}
        {currentPage === "cpu" && systemInfo && <CpuPage cpu={systemInfo.cpu} />}
        {currentPage === "memory" && systemInfo && <MemoryPage memory={systemInfo.memory} />}
        {currentPage === "gpu" && systemInfo && <GpuPage gpu={systemInfo.gpu} />}
        {currentPage === "disk" && systemInfo && <DiskPage disks={systemInfo.disks} throughput={systemInfo.disk_throughput} />}
        {currentPage === "processes" && systemInfo && <ProcessesPage />}
        {currentPage === "settings" && <SettingsPage />}
      </main>
    </div>
    </LanguageContext.Provider>
  );
}

export function StorageRow({ disk }: { disk: PhysicalDisk }) {
  const t = useText();
  const capacity = storageUsage(disk.containers);
  const usage = capacity?.percent ?? null;
  return (
    <div className="storage-row">
      <div className="storage-row-head">
        <Icon name="disk" />
        <div className="storage-name">
          <strong>{disk.name || disk.device}</strong>
          <small>{disk.device}</small>
        </div>
        <div className="storage-usage">
          <span className="label">{t("Usage")}</span>
          {usage === null ? (
            <span className="storage-na">—</span>
          ) : (
            <>
              <div className="progress-bar" role="progressbar" aria-label={t("Usage")} aria-valuemin={0} aria-valuemax={100} aria-valuenow={usage}>
                <div className="progress" style={{ width: `${usage}%` }}></div>
              </div>
              <span className="storage-pct">{usage.toFixed(0)}%</span>
            </>
          )}
        </div>
        <div className="storage-temp">
          <span className="label">{t("Temp")}</span>
          {disk.temperature_celsius === null ? (
            <span className="storage-na">—</span>
          ) : (
            <span className="value">{disk.temperature_celsius.toFixed(1)}°C</span>
          )}
        </div>
      </div>
      <div className="storage-capacity">
        <span>{t("Physical capacity")}: {disk.size_bytes > 0 ? formatBytes(disk.size_bytes) : "—"}</span>
        <span>{t("Used:")} {capacity ? formatBytes(capacity.used) : "—"}</span>
        <small>{t("APFS capacity basis")}: {capacity ? formatBytes(capacity.total) : "—"}</small>
      </div>
      <TempSparkline historyKey={disk.device_uid ?? disk.device} label={disk.device} />
    </div>
  );
}

export function StorageOverview({ storage }: { storage: PhysicalDisk[] }) {
  const t = useText();
  if (storage.length === 0) return null;
  return (
    <div className="card storage-card">
      <div className="card-header">
        <Icon name="disk" />
        <div>
          <h3>{t("Storage Devices")}</h3>
          <small>{storage.length}</small>
        </div>
      </div>
      <div className="storage-rows">
        {orderOverviewDisks(storage).map((d) => (
          <StorageRow key={d.device} disk={d} />
        ))}
      </div>
    </div>
  );
}

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

interface ChartProps {
  history: [number, number][];
  label: string;
  /** Value unit: "%" (0-100 fixed axis), "MB/s" (dynamic axis), etc. */
  unit?: "%" | "MB/s" | "°C";
  /** Gap in seconds beyond which the line breaks instead of interpolating.
   *  Pass gapThresholdSecs(durationSecs) for range-aware semantics (R10). */
  gap_secs?: number;
  /** Fixed x-axis window [start,end] in Unix secs so the requested range is
   *  pinned — leading/trailing empty space stays empty (R10), not stretched. */
  rangeStart?: number;
  rangeEnd?: number;
}

function Chart({ history, label, unit = "%", gap_secs = 5, rangeStart, rangeEnd }: ChartProps) {
  if (history.length === 0) return null;

  const W = 800;
  const H = 220;
  const PAD_TOP = 16;
  const PAD_BOTTOM = 30;

  // Real time axis: x is proportional to timestamp, not array index (F07).
  // When a fixed range is supplied, the axis pins to it so gaps at the edges
  // (stopped sampling, no data yet) render as empty space (R10).
  const ts = history.map(([t]) => t);
  const tMin = rangeStart ?? Math.min(...ts);
  const tMax = rangeEnd ?? Math.max(...ts);
  const tSpan = Math.max(tMax - tMin, 1); // avoid /0 for a single instant

  // Downsample to a vertex budget while preserving first/last and each
  // bucket's min/max, so a spike is never smoothed away and the newest point
  // is never dropped (R10).
  const pts = downsamplePreserveExtremes(history, 400);

  // Y axis: fixed 0-100 for percentages; dynamic for rates/temps (F06).
  const vals = pts.map(([, v]) => v);
  let yMin = 0;
  let yMax = 100;
  if (unit !== "%") {
    yMax = Math.max(...vals, 1);
    // Round the top up to a tidy value for readability.
    const mag = Math.pow(10, Math.floor(Math.log10(yMax)));
    yMax = Math.ceil(yMax / mag) * mag;
  }
  const ySpan = Math.max(yMax - yMin, 1e-9);

  const toX = (t: number) => ((t - tMin) / tSpan) * W;
  const toY = (v: number) => PAD_TOP + (1 - (v - yMin) / ySpan) * (H - PAD_TOP - PAD_BOTTOM);

  // Split into segments wherever the time gap exceeds the (range-aware)
  // threshold, so sleep / collection gaps render as breaks, not connected
  // lines (F07, R10).
  const segments: [number, number][][] = [];
  let current: [number, number][] = [];
  for (let i = 0; i < pts.length; i++) {
    const [t, v] = pts[i];
    if (i > 0 && t - pts[i - 1][0] > gap_secs) {
      if (current.length > 0) segments.push(current);
      current = [];
    }
    current.push([t, v]);
  }
  if (current.length > 0) segments.push(current);

  const singlePoint = pts.length === 1;

  const fmtVal = (v: number) =>
    unit === "%" ? `${v.toFixed(0)}%` : unit === "MB/s" ? `${v.toFixed(0)} MB/s` : `${v.toFixed(1)}°C`;

  return (
    <div className="chart" role="img" aria-label={`${label} history chart, ${history.length} samples${pts.length < history.length ? `, showing ${pts.length}` : ""}`}>
      <svg width="100%" height={H} viewBox={`0 0 ${W} ${H}`} preserveAspectRatio="none">
        <defs>
          <linearGradient id={`${label}-area`} x1="0" x2="0" y1="0" y2="1">
            <stop offset="0%" stopColor="rgba(10,132,255,.28)" />
            <stop offset="100%" stopColor="rgba(10,132,255,.04)" />
          </linearGradient>
        </defs>
        <g stroke="rgba(148,163,184,.18)" strokeWidth="1">
          {[0.25, 0.5, 0.75].map((f) => (
            <line key={f} x1="0" y1={PAD_TOP + f * (H - PAD_TOP - PAD_BOTTOM)} x2={W} y2={PAD_TOP + f * (H - PAD_TOP - PAD_BOTTOM)} />
          ))}
        </g>
        {singlePoint ? (
          // A single sample is a dot, not an area spanning the full width.
          <circle cx={toX(ts[0])} cy={toY(vals[0])} r="4" fill="#0a84ff" />
        ) : (
          segments.map((seg, si) => {
            if (seg.length === 1) {
              return <circle key={si} cx={toX(seg[0][0])} cy={toY(seg[0][1])} r="3" fill="#0a84ff" />;
            }
            const line = seg.map(([t, v], i) => `${i === 0 ? "M" : "L"}${toX(t)},${toY(v)}`).join(" ");
            const firstX = toX(seg[0][0]);
            const lastX = toX(seg[seg.length - 1][0]);
            const area = `${line} L${lastX},${H - PAD_BOTTOM} L${firstX},${H - PAD_BOTTOM} Z`;
            return (
              <g key={si}>
                <path d={area} fill={`url(#${label}-area)`} stroke="none" />
                <path d={line} fill="none" stroke="#0a84ff" strokeWidth="2.5" strokeLinejoin="round" strokeLinecap="round" />
              </g>
            );
          })
        )}
      </svg>
      <div className="chart-labels">
        <span>{fmtVal(yMin)}</span>
        <span>{fmtVal(yMax)}</span>
      </div>
    </div>
  );
}

// Overview temperature history: labelled axes, fixed one-hour window and honest gaps.
export function TempSparkline({ historyKey, label }: { historyKey: string; label: string }) {
  const { points, status } = useHistoryQuery("disk.temperature", historyKey, 3600);
  const language = useContext(LanguageContext);
  return <TemperatureTrend key={historyKey} history={points} status={status} label={label} english={language === "en"} />;
}

function CpuPage({ cpu }: { cpu: CpuInfo }) {
  const t = useText();
  const { points: history, status: histStatus, loaded: histLoaded } = useHistoryQuery("cpu.total_usage", "system", 3600);

  return (
    <div>
      <h2>{t("CPU Details")}</h2>
      <div className="card">
        <h3>{cpu.name}</h3>
        <div className="info">
          <div className="label">{t("Physical Cores:")}</div>
          <div className="value">{cpu.physical_cores}</div>
        </div>
        <div className="info">
          <div className="label">{t("Logical Processors:")}</div>
          <div className="value">{cpu.logical_cores}</div>
        </div>
        <div className="info">
          <div className="label">{t("Total Usage:")}</div>
          <div className="value">{cpu.total_usage.toFixed(1)}%</div>
        </div>
        <div className="progress-bar">
          <div className="progress" style={{ width: `${cpu.total_usage}%` }}></div>
        </div>
      </div>

      <div className="card">
        <h3>{t("Per-Core Usage")}</h3>
        <div className="core-grid">
          {cpu.per_core_usage.map((usage, idx) => (
            <div key={idx} className="core-item">
              <div className="core-label">{t("CPU")} {idx}</div>
              <div className="core-bar">
                <div className="core-progress" style={{ width: `${usage}%` }}></div>
              </div>
              <div className="core-value">{usage.toFixed(0)}%</div>
            </div>
          ))}
        </div>
      </div>

      <div className="card">
        <h3>{t("Usage History (Last Hour)")}</h3>
        {histStatus === "error" && <p className="note" role="status">{t("Failed to load history")}</p>}
        {histStatus !== "error" && histLoaded && history.length === 0 && <p className="note">{t("No history yet")}</p>}
        {history.length > 0 && <Chart history={history} label="cpu" gap_secs={gapThresholdSecs(3600)} />}
      </div>
    </div>
  );
}

function MemoryPage({ memory }: { memory: MemoryInfo }) {
  const t = useText();
  const { points: history, status: histStatus, loaded: histLoaded } = useHistoryQuery("memory.used_percent", "system", 3600);

  return (
    <div>
      <h2>{t("Memory Details")}</h2>
      <div className="card">
        <h3>{t("System Memory")}</h3>
        <div className="info">
          <div className="label">{t("Total:")}</div>
          <div className="value">{formatBytes(memory.total_bytes)}</div>
        </div>
        <div className="info">
          <div className="label">{t("Used:")}</div>
          <div className="value">{formatBytes(memory.used_bytes)}</div>
        </div>
        <div className="info">
          <div className="label">{t("Available:")}</div>
          <div className="value">{formatBytes(memory.available_bytes)}</div>
        </div>
        <div className="info">
          <div className="label">{t("Usage:")}</div>
          <div className="value">{memory.used_percent.toFixed(1)}%</div>
        </div>
        <div className="progress-bar">
          <div className="progress" style={{ width: `${memory.used_percent}%` }}></div>
        </div>
      </div>

      <div className="card">
        <h3>{t("Usage History (Last Hour)")}</h3>
        {histStatus === "error" && <p className="note" role="status">{t("Failed to load history")}</p>}
        {histStatus !== "error" && histLoaded && history.length === 0 && <p className="note">{t("No history yet")}</p>}
        {history.length > 0 && <Chart history={history} label="memory" gap_secs={gapThresholdSecs(3600)} />}
      </div>
    </div>
  );
}

function GpuPage({ gpu }: { gpu: GpuInfo }) {
  const t = useText();
  const { points: history, status: histStatus, loaded: histLoaded } = useHistoryQuery("gpu.utilization", "gpu0", 3600);

  return (
    <div>
      <h2>{t("GPU Details")}</h2>
      <div className="card">
        <h3>{gpu.name}</h3>
        <div className="info">
          <div className="label">{t("Utilization:")}</div>
          <div className="value">{gpu.utilization.toFixed(1)}%</div>
        </div>
        <div className="progress-bar">
          <div className="progress" style={{ width: `${gpu.utilization}%` }}></div>
        </div>
      </div>

      <div className="card">
        <h3>{t("Memory")}</h3>
        <div className="info">
          <div className="label">{t("In Use:")}</div>
          <div className="value">{formatBytes(gpu.memory_used_bytes)}</div>
        </div>
        <div className="info">
          <div className="label">{t("Allocated:")}</div>
          <div className="value">{formatBytes(gpu.memory_allocated_bytes)}</div>
        </div>
        <p className="note">{t("Unified memory architecture - no separate VRAM")}</p>
      </div>

      <div className="card">
        <h3>{t("Usage History (Last Hour)")}</h3>
        {histStatus === "error" && <p className="note" role="status">{t("Failed to load history")}</p>}
        {histStatus !== "error" && histLoaded && history.length === 0 && <p className="note">{t("No history yet")}</p>}
        {history.length > 0 && <Chart history={history} label="gpu" gap_secs={gapThresholdSecs(3600)} />}
      </div>
    </div>
  );
}

function DiskPage({ disks, throughput }: { disks: DiskInfo[]; throughput: DiskThroughput[] }) {
  const t = useText();
  // Track which disk's throughput history is shown; default to the first.
  // Selection and history are keyed by the stable anonymous device_uid (A10),
  // never the volatile diskN address, so hot-plug/reboot cannot cross-join.
  const [selected, setSelected] = useState<string>("");
  // R10: 1h / 24h / 7d range switcher.
  const [rangeSecs, setRangeSecs] = useState(3600);

  const historyKeyOf = (d: DiskInfo) => d.device_uid || d.device;
  const activeKey = selected || (disks[0] ? historyKeyOf(disks[0]) : "");
  const activeDisk = disks.find((d) => historyKeyOf(d) === activeKey);
  const activeLabel = activeDisk ? activeDisk.name || activeDisk.device : "";

  // Single-flight query keyed by (metric, objectId=uid, range): switching disk
  // or range cancels the prior flight and clears the curve, so a stale or
  // previous-disk series is never rendered (R10 cache isolation).
  const { points: history, status: histStatus, loaded: histLoaded } =
    useHistoryQuery("disk.throughput", activeKey, rangeSecs);

  // Pin the x-axis to the requested window so leading/trailing gaps stay empty.
  const nowSecs = Math.floor(Date.now() / 1000);
  const rangeStart = nowSecs - rangeSecs;
  const rangeEnd = nowSecs;

  return (
    <div>
      <h2>{t("Disk Details")}</h2>
      {disks.map((disk) => {
        const diskThroughput = throughput.find((t) => t.device === disk.device);
        return (
          <div key={historyKeyOf(disk)} className="card">
            <h3>{disk.name || disk.device}</h3>
            <div className="info">
              <div className="label">{t("Device:")}</div>
              <div className="value">{disk.device}</div>
            </div>
            <div className="info">
              <div className="label">{t("Capacity:")}</div>
              <div className="value">{formatBytes(disk.size_bytes)}</div>
            </div>
            <div className="info">
              <div className="label">{t("SMART Status:")}</div>
              <div className="value">{disk.smart_status}</div>
            </div>
            {disk.temperature_celsius !== null && (
              <div className="info">
                <div className="label">{t("Temperature:")}</div>
                <div className="value">{disk.temperature_celsius.toFixed(1)}°C</div>
              </div>
            )}
            {disk.power_on_hours !== null && (
              <div className="info">
                <div className="label">{t("Power On Hours:")}</div>
                <div className="value">{disk.power_on_hours} {t("hours")}</div>
              </div>
            )}
            {diskThroughput && (
              <div className="info">
                <div className="label">{t("Throughput:")}</div>
                <div className="value">{diskThroughput.mb_per_sec.toFixed(2)} MB/s</div>
              </div>
            )}
          </div>
        );
      })}

      {disks.length > 1 && (
        <div className="controls">
          {disks.map((d) => (
            <div className="sort-buttons" key={historyKeyOf(d)}>
              <button
                className={activeKey === historyKeyOf(d) ? "active" : ""}
                onClick={() => setSelected(historyKeyOf(d))}
              >
                {d.name || d.device}
              </button>
            </div>
          ))}
        </div>
      )}

      <div className="controls">
        <div className="sort-buttons" role="group" aria-label={t("History range")}>
          {([["1h", 3600], ["24h", 86400], ["7d", 604800]] as const).map(([lbl, secs]) => (
            <button key={lbl} className={rangeSecs === secs ? "active" : ""} onClick={() => setRangeSecs(secs)}>{lbl}</button>
          ))}
        </div>
      </div>

      <div className="card">
        <h3>{t("Throughput History")}{activeLabel ? ` · ${activeLabel}` : ""}</h3>
        {histStatus === "error" && <p className="note" role="status">{t("Failed to load history")}</p>}
        {histStatus !== "error" && histLoaded && history.length === 0 && <p className="note">{t("No history yet")}</p>}
        {history.length > 0 && (
          <Chart history={history} label="disk" unit="MB/s" gap_secs={gapThresholdSecs(rangeSecs)} rangeStart={rangeStart} rangeEnd={rangeEnd} />
        )}
      </div>
    </div>
  );
}

export function ProcessesPage() {
  const t = useText();
  const [page, setPage] = useState<ProcessPage | null>(null);
  const [sortBy, setSortBy] = useState<ProcessSortKey>("memory");
  const [searchTerm, setSearchTerm] = useState("");
  const [debouncedSearch, setDebouncedSearch] = useState("");
  const [pageIndex, setPageIndex] = useState(0);
  const [pageSize, setPageSize] = useState(50);
  const [loadError, setLoadError] = useState(false);
  const [terminationMessage, setTerminationMessage] = useState("");

  // R8 termination state machine. The target is an immutable snapshot taken at
  // confirm time; later list refreshes can never mutate what would be killed.
  type TermState =
    | { phase: "idle" }
    | { phase: "selected"; target: ProcessInfo }
    | { phase: "confirming"; target: ProcessInfo }
    | { phase: "sending"; target: ProcessInfo };
  const [term, setTerm] = useState<TermState>({ phase: "idle" });
  const selected = term.phase === "idle" ? null : term.target;
  const confirming = term.phase === "confirming" || term.phase === "sending";
  const sending = term.phase === "sending";
  const endButtonRef = useRef<HTMLButtonElement | null>(null);
  const cancelRef = useRef<HTMLButtonElement | null>(null);

  const cancelConfirm = (restoreFocus: boolean) => {
    setTerm((prev) => (prev.phase === "confirming" || prev.phase === "sending" ? { phase: "selected", target: prev.target } : prev));
    if (restoreFocus) endButtonRef.current?.focus();
  };

  const endProcess = async () => {
    if (term.phase !== "confirming") return; // only a confirming target can be sent
    const target = term.target; // immutable snapshot
    setTerm({ phase: "sending", target });
    try {
      await invoke("terminate_process", { pid: target.pid, startMarker: target.start_marker });
      setTerminationMessage(t("Termination requested; process may still be running."));
      setTerm({ phase: "idle" });
    } catch (error) {
      const messages: Record<string, string> = {
        protected: "This process is protected.", gone: "Process already exited.",
        changed: "Process identity changed. Select it again.",
        permission: "Permission denied; only your own processes can be ended.",
      };
      setTerminationMessage(t(messages[String(error)] ?? "Failed to request termination."));
      setTerm({ phase: "idle" });
    }
  };

  // R6: debounce the search so each keystroke does not trigger a re-scan.
  useEffect(() => {
    const h = setTimeout(() => { setDebouncedSearch(searchTerm); setPageIndex(0); }, 300);
    return () => clearTimeout(h);
  }, [searchTerm]);

  // Reset to page 0 whenever sort or page size changes.
  useEffect(() => { setPageIndex(0); }, [sortBy, pageSize]);

  useEffect(() => {
    let cancelled = false;
    const fetchProcesses = async () => {
      try {
        const result = await invoke<ProcessPage>("get_processes", {
          search: debouncedSearch || null,
          sort: sortBy,
          offset: pageIndex * pageSize,
          limit: pageSize,
        });
        if (!cancelled) {
          setPage(result);
          setLoadError(false);
        }
      } catch (err) {
        if (!cancelled) {
          setLoadError(true);
        }
        console.error("Failed to fetch processes:", err);
      }
    };

    fetchProcesses();
    const interval = setInterval(fetchProcesses, 2000);
    return () => {
      cancelled = true;
      clearInterval(interval);
    };
  }, [debouncedSearch, sortBy, pageIndex, pageSize]);

  const processes = page?.processes ?? [];

  // R8 staleness guard: whenever the visible list, the filter/sort/page, or the
  // load state changes, a selected/confirming target that is no longer present
  // in the current page is cancelled — a stale selection can never be sent.
  useEffect(() => {
    if (term.phase === "idle") return;
    if (loadError) { setTerm({ phase: "idle" }); return; }
    const present = processes.some(
      (p) => p.pid === term.target.pid && p.start_marker === term.target.start_marker
    );
    if (!present) {
      setTerm({ phase: "idle" });
      setTerminationMessage(t("Selection left the current list; select it again."));
    }
  }, [processes, loadError, term]);

  // Esc cancels an open confirm dialog.
  useEffect(() => {
    if (!confirming) return;
    const onKey = (e: KeyboardEvent) => { if (e.key === "Escape") cancelConfirm(true); };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [confirming]);

  // Default focus to Cancel when the dialog opens (safe default).
  useEffect(() => {
    if (confirming) cancelRef.current?.focus();
  }, [confirming]);

  // R6: unknown rate (None) renders "—"; a never-zero fake is never shown.
  const fmtRate = (proc: ProcessInfo, bps: number | null) =>
    !proc.io_ok ? t("Unreadable") : bps === null ? "—" : `${formatBytes(Math.round(bps))}/s`;
  const matched = page?.matched_total ?? 0;
  const totalPages = Math.max(1, Math.ceil(matched / pageSize));

  return (
    <div>
      <h2>{t("Process Ranking")}</h2>
      <div className="card">
        <div className="controls">
          <input
            type="text"
            aria-label={t("Name")} placeholder={t("Name")}
            value={searchTerm}
            onChange={(e) => setSearchTerm(e.target.value)}
            className="search-input"
          />
          <div className="sort-buttons">
            <button className={sortBy === "memory" ? "active" : ""} onClick={() => setSortBy("memory")}>
              {t("Sort by Memory")}
            </button>
            <button className={sortBy === "cpu" ? "active" : ""} onClick={() => setSortBy("cpu")}>
              {t("Sort by CPU")}
            </button>
            <button className={sortBy === "diskread" ? "active" : ""} onClick={() => setSortBy("diskread")}>
              {t("Sort by Read")}
            </button>
            <button className={sortBy === "diskwrite" ? "active" : ""} onClick={() => setSortBy("diskwrite")}>
              {t("Sort by Write")}
            </button>
          </div>
        </div>
        {page && (
          <p className="note">
            {t("Showing")} {processes.length} {t("of")} {matched} {t("matching")} · {page.total_readable} {t("readable processes (system-wide disk I/O)")}
          </p>
        )}
        {loadError && <p className="note">{t("Failed to load processes")}</p>}
        <div className="pagination-controls">
          <button disabled={pageIndex === 0} onClick={() => setPageIndex(0)}>«</button>
          <button disabled={pageIndex === 0} onClick={() => setPageIndex(i => Math.max(0, i - 1))}>{t("Prev")}</button>
          <span>{t("Page")} {matched === 0 ? 0 : pageIndex + 1} {t("of")} {matched === 0 ? 0 : totalPages}</span>
          <button disabled={pageIndex + 1 >= totalPages} onClick={() => setPageIndex(i => Math.min(totalPages - 1, i + 1))}>{t("Next")}</button>
          <button disabled={pageIndex + 1 >= totalPages} onClick={() => setPageIndex(totalPages - 1)}>»</button>
          <select aria-label={t("Rows per page")} value={pageSize} onChange={(e) => setPageSize(Number(e.target.value))}>
            {[25, 50, 100, 200].map(n => <option key={n} value={n}>{n} / {t("page")}</option>)}
          </select>
        </div>
        <div className="process-actions">
          <button ref={endButtonRef} className="danger-button" disabled={!selected || confirming || loadError} onClick={() => selected && setTerm({ phase: "confirming", target: selected })}>{t("End process")}</button>
          <span>{selected ? `${selected.name} · PID ${selected.pid}` : t("Select a process")}</span>
        </div>
        {terminationMessage && <p role="status" className="note">{terminationMessage}</p>}
        {confirming && selected && <div className="confirm-panel" role="alertdialog" aria-modal="false" aria-labelledby="end-title" aria-describedby="end-description">
          <h3 id="end-title">{t("End process")}: {selected.name} · PID {selected.pid}</h3>
          <p id="end-description">{t("Unsaved work may be lost. Send SIGTERM without force or elevation?")}</p>
          <button ref={cancelRef} disabled={sending} onClick={() => cancelConfirm(true)}>{t("Cancel")}</button>
          <button className="danger-button" disabled={sending} onClick={endProcess}>{t(sending ? "Requesting…" : "Confirm termination")}</button>
        </div>}
        <div className="process-table-wrapper">
        <table className="process-table">
          <thead>
            <tr>
              <th scope="col">PID</th>
              <th scope="col">{t("Name")}</th>
              <th scope="col">{t("Memory")}</th>
              <th scope="col">CPU %</th>
              <th scope="col">{t("Read/s")}</th>
              <th scope="col">{t("Write/s")}</th>
            </tr>
          </thead>
          <tbody>
            {processes.length === 0 && !loadError && (
              <tr><td colSpan={6}>{t("No matching processes")}</td></tr>
            )}
            {processes.map((proc) => (
              <tr key={`${proc.pid}-${proc.start_marker}`} className={selected?.pid === proc.pid && selected.start_marker === proc.start_marker ? "selected" : ""}>

                <td><input type="radio" name="selected-process" aria-label={`${t("Select a process")}: ${proc.name} PID ${proc.pid}`} disabled={confirming} checked={selected?.pid === proc.pid && selected.start_marker === proc.start_marker} onChange={() => {setTerm({ phase: "selected", target: proc }); setTerminationMessage("");}} />{proc.pid}</td>
                <td>{proc.name}</td>
                <td>{formatBytes(proc.memory_bytes)}</td>
                <td>{proc.cpu_usage.toFixed(1)}%</td>
                <td>{fmtRate(proc, proc.disk_read_bps)}</td>
                <td>{fmtRate(proc, proc.disk_write_bps)}</td>
              </tr>
            ))}
          </tbody>
        </table>
        </div>
      </div>
    </div>
  );
}

interface AppSettings {
  version: number;
  foreground_interval_ms: number;
  background_interval_ms: number;
  launch_at_login: boolean;
  language: string;
}

/** R9/A13: settings payload with any load/validation error surfaced. */
interface SettingsPayload {
  settings: AppSettings;
  load_error: string | null;
}

/** R9/A12: verified login-item result; registered=null means unknown. */
interface LoginItemResult {
  registered: boolean | null;
  saved: boolean;
  error: string | null;
}

function SettingsPage() {
  const t = useText();
  // R9/A13: draft (what the user is editing) is separate from the last saved
  // value; submission is debounced so a burst of keystrokes never sends an
  // intermediate/0 value, and a failed save shows an error with the draft kept.
  const [_saved, setSaved] = useState<AppSettings | null>(null);
  const [draft, setDraft] = useState<AppSettings | null>(null);
  const [saveState, setSaveState] = useState<"idle" | "saving" | "saved" | "error">("idle");
  const [errorMsg, setErrorMsg] = useState("");
  const [loadError, setLoadError] = useState<string | null>(null);
  const [loginNote, setLoginNote] = useState<string | null>(null);
  const saveTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  // Serialize saves: only the latest draft is sent after the in-flight one
  // resolves, so an older slower response can never overwrite a newer value.
  const savingRef = useRef(false);
  const pendingRef = useRef<AppSettings | null>(null);

  useEffect(() => {
    if (!isTauri()) return;
    invoke<SettingsPayload>("get_settings")
      .then((p) => {
        setSaved(p.settings);
        setDraft(p.settings);
        setLoadError(p.load_error);
      })
      .catch((e) => {
        // First-load failure must be visible, not an endless "Loading…" (A13).
        setLoadError(String(e));
        setSaved(null);
        setDraft(null);
      });
  }, []);

  const flushSave = async (next: AppSettings) => {
    if (savingRef.current) { pendingRef.current = next; return; }
    savingRef.current = true;
    setSaveState("saving");
    try {
      await invoke("set_settings", { newSettings: next });
      setSaved(next);
      setSaveState("saved");
    } catch (e) {
      setSaveState("error");
      setErrorMsg(String(e));
    } finally {
      savingRef.current = false;
      if (pendingRef.current) {
        const p = pendingRef.current;
        pendingRef.current = null;
        flushSave(p);
      }
    }
  };

  const edit = (patch: Partial<AppSettings>) => {
    if (!draft) return;
    const next = { ...draft, ...patch };
    setDraft(next);
    if (saveTimer.current) clearTimeout(saveTimer.current);
    saveTimer.current = setTimeout(() => flushSave(next), 500);
  };

  const toggleLogin = async (enable: boolean) => {
    if (!draft) return;
    setLoginNote(null);
    try {
      const r = await invoke<LoginItemResult>("set_launch_at_login", { enable });
      if (r.registered === null) {
        // Verification failed: do not change the toggle to a guessed state.
        setLoginNote(t("Could not verify login item state; left unchanged."));
        return;
      }
      const next = { ...draft, launch_at_login: r.registered };
      setDraft(next);
      setSaved(next);
      if (!r.saved) setLoginNote(t("Login item changed but settings were not saved."));
    } catch (e) {
      setSaveState("error");
      setErrorMsg(String(e));
    }
  };

  if (!isTauri()) {
    return (
      <div>
        <h2>{t("Settings")}</h2>
        <div className="card"><p className="note">{t("Settings are available in the desktop app")}</p></div>
      </div>
    );
  }
  if (!draft) {
    return (
      <div>
        <h2>{t("Settings")}</h2>
        <div className="card">
          {loadError
            ? <p className="note" role="alert">{t("Failed to load settings")}: {loadError}</p>
            : <p className="note">{t("Loading settings…")}</p>}
        </div>
      </div>
    );
  }

  const shown = draft;
  return (
    <div>
      <h2>{t("Settings")}</h2>
      {loadError && <p className="note" role="alert">{t("Settings file was invalid; defaults restored. The original file was kept.")} ({loadError})</p>}
      <div className="card">
        <h3>{t("Sampling")}</h3>
        <div className="info">
          <div className="label">{t("Foreground interval (ms)")}</div>
          <div className="value">
            <input
              type="number" min={500} max={10000} step={100}
              value={shown.foreground_interval_ms}
              onChange={(e) => edit({ foreground_interval_ms: Number(e.target.value) })}
              className="search-input" style={{ maxWidth: 120 }}
            />
          </div>
        </div>
        <div className="info">
          <div className="label">{t("Background interval (ms)")}</div>
          <div className="value">
            <input
              type="number" min={1000} max={30000} step={500}
              value={shown.background_interval_ms}
              onChange={(e) => edit({ background_interval_ms: Number(e.target.value) })}
              className="search-input" style={{ maxWidth: 120 }}
            />
          </div>
        </div>
      </div>

      <div className="card">
        <h3>{t("Startup")}</h3>
        <div className="info">
          <div className="label">{t("Launch at login")}</div>
          <div className="value">
            <button
              className={shown.launch_at_login ? "active" : ""}
              onClick={() => toggleLogin(!shown.launch_at_login)}
            >
              {shown.launch_at_login ? t("On") : t("Off")}
            </button>
          </div>
        </div>
        <p className="note">{t("Closing the window keeps monitoring in the menu bar; Quit stops collection.")}</p>
        {loginNote && <p className="note" role="status">{loginNote}</p>}
      </div>

      {saveState === "saved" && <p className="note">{t("Settings saved")}</p>}
      {saveState === "error" && <p className="note" role="alert">{t("Failed to save settings")} {errorMsg}</p>}
    </div>
  );
}

export default App;