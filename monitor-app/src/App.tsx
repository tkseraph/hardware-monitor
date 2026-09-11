import { createContext, useContext, useEffect, useState } from "react";
import { invoke, isTauri } from "@tauri-apps/api/core";

type Language = "zh" | "en";
const LanguageContext = createContext<Language>("zh");
const translations: Record<string, string> = {"System Overview": "系统总览", "CPU Details": "处理器详情", "Memory Details": "内存详情", "GPU Details": "图形处理器详情", "Disk Details": "磁盘详情", "Process Ranking": "进程排行", "Settings": "设置", "No matching processes": "暂无匹配的进程", "General": "通用", "CPU": "处理器", "GPU": "图形处理器", "Memory": "内存", "Disks": "存储设备", "Name:": "名称", "Cores:": "核心数量", "Usage:": "使用率", "Total:": "总容量", "Used:": "已使用", "Utilization:": "利用率", "Memory:": "内存用量", "Active I/O:": "有吞吐的设备", "Physical Cores:": "物理核心", "Logical Processors:": "逻辑处理器", "Total Usage:": "总使用率", "Per-Core Usage": "逐核使用率", "Usage History (Last Hour)": "使用率历史 · 最近一小时", "System Memory": "系统内存", "Available:": "可用", "In Use:": "使用中", "Allocated:": "已分配", "Unified memory architecture - no separate VRAM": "统一内存架构，无独立显存；以下为驱动统计，不代表独立显存容量。", "Device:": "设备标识", "Capacity:": "容量", "SMART Status:": "SMART 摘要", "Temperature:": "温度", "Power On Hours:": "通电时间", "hours": "小时", "Throughput:": "合计吞吐", "Throughput History (Last Hour)": "吞吐历史 · 最近一小时", "Name": "进程名称", "Sort by Memory": "按内存排序", "Sort by CPU": "按 CPU 排序", "Sort by Read": "按读取排序", "Sort by Write": "按写入排序", "Showing": "显示", "of": "共", "readable processes (system-wide disk I/O)": "个可读取进程（磁盘读写为系统范围）", "Failed to load processes": "进程加载失败", "Read/s": "读取/秒", "Write/s": "写入/秒", "Settings will be implemented in a future update.": "采样频率、历史保留与登录项设置尚未实现。", "Sampling": "采样", "Foreground interval (ms)": "前台采样间隔（毫秒）", "Background interval (ms)": "后台采样间隔（毫秒）", "Startup": "启动", "Launch at login": "登录时启动", "On": "开", "Off": "关", "Closing the window keeps monitoring in the menu bar; Quit stops collection.": "关闭窗口后在菜单栏继续采集；选择退出才停止。", "Settings saved": "设置已保存", "Failed to save settings": "设置保存失败", "Settings are available in the desktop app": "设置仅在桌面应用中可用", "Loading settings…": "正在加载设置…"};
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
  name: string;
  size_bytes: number;
  smart_status: string;
  temperature_celsius: number | null;
  power_on_hours: number | null;
}

interface DiskThroughput {
  device: string;
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
}

interface ProcessPage {
  processes: ProcessInfo[];
  total_readable: number;
  observed_at: number;
}

type ProcessSortKey = "memory" | "cpu" | "diskread" | "diskwrite";

interface VolumeInfo {
  id: string;
  name: string;
  role: string;
  capacity_consumed: number | null;
}

interface ContainerInfo {
  container_ref: string;
  physical_store: string | null;
  capacity_ceiling: number | null;
  capacity_free: number | null;
  capacity_in_use: number | null;
  volumes: VolumeInfo[];
}

interface PhysicalDisk {
  device: string;
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

function App() {
  const [language, setLanguage] = useState<Language>(() => localStorage.getItem("monitor-language") === "en" ? "en" : "zh");
  const zh = language === "zh";
  useEffect(() => { localStorage.setItem("monitor-language", language); document.documentElement.lang = zh ? "zh-CN" : "en"; }, [language, zh]);
  const [systemInfo, setSystemInfo] = useState<SystemInfo | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [currentPage, setCurrentPage] = useState<Page>("overview");

  useEffect(() => {
    if (!isTauri()) return;
    let busy = false;
    const fetchData = async () => {
      if (busy) return;
      busy = true;
      try {
        const info = await invoke<SystemInfo>("get_system_info");
        setSystemInfo(info);
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
        <header className="topbar"><span>Monitor <span className="crumb">/ {pages.find(p => p[0] === currentPage)?.[zh ? 1 : 2]}</span></span><select aria-label={zh ? "界面语言" : "Language"} value={language} onChange={e => setLanguage(e.target.value as Language)}><option value="zh">简体中文</option><option value="en">English</option></select></header>
        <section className="page-heading"><div><div className="eyebrow">HARDWARE MONITOR</div><h2>{zh ? "洞悉设备的每一刻" : "Your hardware, at a glance"}</h2><p>{zh ? "专注关键指标，让系统状态清晰可见。" : "A clear view of the metrics that matter."}</p></div><span className="status-pill">{!isTauri() ? (zh ? "浏览器预览" : "Browser preview") : error ? (zh ? "采集异常" : "Collection error") : systemInfo ? (zh ? "实时采集中" : "Collecting") : (zh ? "等待采样" : "Waiting")}</span></section>
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

        <div className="card hero-card">
          <div className="card-header">
            <Icon name="disk" />
            <div>
              <h3>{t("Disks")}</h3>
              <small>{systemInfo.disks.length} {t("Disks")}</small>
            </div>
          </div>
          <div className="big-value">
            {systemInfo.disk_throughput.filter((t) => t.mb_per_sec > 0).length}
          </div>
          <div className="progress-bar">
            <div className="progress" style={{ width: `${(systemInfo.disk_throughput.filter((t) => t.mb_per_sec > 0).length / Math.max(systemInfo.disks.length, 1)) * 100}%` }}></div>
          </div>
          <div className="info-row">
            <span>{t("Active I/O:")}</span>
          </div>
        </div>
      </div>
    </div>
  );
}

interface ChartProps {
  history: [number, number][];
  label: string;
  /** Value unit: "%" (0-100 fixed axis), "MB/s" (dynamic axis), etc. */
  unit?: "%" | "MB/s" | "°C";
  /** Gap in seconds beyond which the line breaks instead of interpolating. */
  gap_secs?: number;
}

function Chart({ history, label, unit = "%", gap_secs = 5 }: ChartProps) {
  if (history.length === 0) return null;

  const W = 800;
  const H = 220;
  const PAD_TOP = 16;
  const PAD_BOTTOM = 30;

  // Real time axis: x is proportional to timestamp, not array index (F07).
  const ts = history.map(([t]) => t);
  const tMin = Math.min(...ts);
  const tMax = Math.max(...ts);
  const tSpan = Math.max(tMax - tMin, 1); // avoid /0 for a single instant

  // Y axis: fixed 0-100 for percentages; dynamic for rates/temps (F06).
  const vals = history.map(([, v]) => v);
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

  // Split into segments wherever the time gap exceeds the sampling cadence,
  // so sleep / collection gaps render as breaks, not connected lines (F07).
  const segments: [number, number][][] = [];
  let current: [number, number][] = [];
  for (let i = 0; i < history.length; i++) {
    const [t, v] = history[i];
    if (i > 0 && t - history[i - 1][0] > gap_secs) {
      if (current.length > 0) segments.push(current);
      current = [];
    }
    current.push([t, v]);
  }
  if (current.length > 0) segments.push(current);

  const singlePoint = history.length === 1;

  const fmtVal = (v: number) =>
    unit === "%" ? `${v.toFixed(0)}%` : unit === "MB/s" ? `${v.toFixed(0)} MB/s` : `${v.toFixed(1)}°C`;

  return (
    <div className="chart" role="img" aria-label={`${label} history chart, ${history.length} samples`}>
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

// Per-disk temperature sparkline for the overview storage row. Returns
// null when there is no history (or the fetch fails) so no empty frame
// is rendered — honest absence, never a fabricated zero.
export function TempSparkline({ device }: { device: string }) {
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

function CpuPage({ cpu }: { cpu: CpuInfo }) {
  const t = useText();
  const [history, setHistory] = useState<[number, number][]>([]);

  useEffect(() => {
    const fetchHistory = async () => {
      try {
        const data = await invoke<[number, number][]>("get_history", {
          metricId: "cpu.total_usage",
          objectId: "system",
          durationSecs: 3600, // Last 1 hour
        });
        setHistory(data);
      } catch (err) {
        console.error("Failed to fetch history:", err);
      }
    };

    fetchHistory();
    const interval = setInterval(fetchHistory, 5000);
    return () => clearInterval(interval);
  }, []);

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

      {history.length > 0 && (
        <div className="card">
          <h3>{t("Usage History (Last Hour)")}</h3>
          <Chart history={history} label="cpu" />
        </div>
      )}
    </div>
  );
}

function MemoryPage({ memory }: { memory: MemoryInfo }) {
  const t = useText();
  const [history, setHistory] = useState<[number, number][]>([]);

  useEffect(() => {
    const fetchHistory = async () => {
      try {
        const data = await invoke<[number, number][]>("get_history", {
          metricId: "memory.used_percent",
          objectId: "system",
          durationSecs: 3600,
        });
        setHistory(data);
      } catch (err) {
        console.error("Failed to fetch history:", err);
      }
    };

    fetchHistory();
    const interval = setInterval(fetchHistory, 5000);
    return () => clearInterval(interval);
  }, []);

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

      {history.length > 0 && (
        <div className="card">
          <h3>{t("Usage History (Last Hour)")}</h3>
          <Chart history={history} label="memory" />
        </div>
      )}
    </div>
  );
}

function GpuPage({ gpu }: { gpu: GpuInfo }) {
  const t = useText();
  const [history, setHistory] = useState<[number, number][]>([]);

  useEffect(() => {
    const fetchHistory = async () => {
      try {
        const data = await invoke<[number, number][]>("get_history", {
          metricId: "gpu.utilization",
          objectId: "gpu0",
          durationSecs: 3600,
        });
        setHistory(data);
      } catch (err) {
        console.error("Failed to fetch history:", err);
      }
    };

    fetchHistory();
    const interval = setInterval(fetchHistory, 5000);
    return () => clearInterval(interval);
  }, []);

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

      {history.length > 0 && (
        <div className="card">
          <h3>{t("Usage History (Last Hour)")}</h3>
          <Chart history={history} label="gpu" />
        </div>
      )}
    </div>
  );
}

function DiskPage({ disks, throughput }: { disks: DiskInfo[]; throughput: DiskThroughput[] }) {
  const t = useText();
  const [history, setHistory] = useState<[number, number][]>([]);
  // Track which disk's throughput history is shown; default to the first.
  const [selected, setSelected] = useState<string>("");

  const activeDevice = selected || disks[0]?.device || "";

  useEffect(() => {
    if (!activeDevice) return;
    let cancelled = false;
    const fetchHistory = async () => {
      try {
        const data = await invoke<[number, number][]>("get_history", {
          metricId: "disk.throughput",
          objectId: activeDevice, // per-selected-disk history, not always disk0 (F06)
          durationSecs: 3600,
        });
        if (!cancelled) setHistory(data);
      } catch (err) {
        console.error("Failed to fetch history:", err);
      }
    };

    fetchHistory();
    const interval = setInterval(fetchHistory, 5000);
    return () => {
      cancelled = true;
      clearInterval(interval);
    };
  }, [activeDevice]);

  return (
    <div>
      <h2>{t("Disk Details")}</h2>
      {disks.map((disk) => {
        const diskThroughput = throughput.find((t) => t.device === disk.device);
        return (
          <div key={disk.device} className="card">
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
            <div className="sort-buttons" key={d.device}>
              <button
                className={activeDevice === d.device ? "active" : ""}
                onClick={() => setSelected(d.device)}
              >
                {d.name || d.device}
              </button>
            </div>
          ))}
        </div>
      )}

      {history.length > 0 && (
        <div className="card">
          <h3>{t("Throughput History (Last Hour)")}{activeDevice ? ` · ${activeDevice}` : ""}</h3>
          <Chart history={history} label="disk" unit="MB/s" gap_secs={10} />
        </div>
      )}
    </div>
  );
}

function ProcessesPage() {
  const t = useText();
  const [page, setPage] = useState<ProcessPage | null>(null);
  const [sortBy, setSortBy] = useState<ProcessSortKey>("memory");
  const [searchTerm, setSearchTerm] = useState("");
  const [loadError, setLoadError] = useState(false);

  useEffect(() => {
    let cancelled = false;
    const fetchProcesses = async () => {
      try {
        // Backend filters + sorts + paginates over the FULL readable set,
        // so search and CPU/disk sort are not limited to a memory top-50.
        const result = await invoke<ProcessPage>("get_processes", {
          search: searchTerm || null,
          sort: sortBy,
          offset: 0,
          limit: 50,
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
  }, [searchTerm, sortBy]);

  const processes = page?.processes ?? [];
  const fmtRate = (bps: number | null) =>
    bps === null ? "—" : `${formatBytes(Math.round(bps))}/s`;

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
            {t("Showing")} {processes.length} {t("of")} {page.total_readable} {t("readable processes (system-wide disk I/O)")}
          </p>
        )}
        {loadError && <p className="note">{t("Failed to load processes")}</p>}
        <table className="process-table">
          <thead>
            <tr>
              <th>PID</th>
              <th>{t("Name")}</th>
              <th>{t("Memory")}</th>
              <th>CPU %</th>
              <th>{t("Read/s")}</th>
              <th>{t("Write/s")}</th>
            </tr>
          </thead>
          <tbody>
            {processes.length === 0 && !loadError && (
              <tr><td colSpan={6}>{t("No matching processes")}</td></tr>
            )}
            {processes.map((proc) => (
              <tr key={`${proc.pid}-${proc.start_marker}`}>
                <td>{proc.pid}</td>
                <td>{proc.name}</td>
                <td>{formatBytes(proc.memory_bytes)}</td>
                <td>{proc.cpu_usage.toFixed(1)}%</td>
                <td>{fmtRate(proc.disk_read_bps)}</td>
                <td>{fmtRate(proc.disk_write_bps)}</td>
              </tr>
            ))}
          </tbody>
        </table>
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

function SettingsPage() {
  const t = useText();
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [saveState, setSaveState] = useState<"idle" | "saving" | "saved" | "error">("idle");
  const [errorMsg, setErrorMsg] = useState("");

  useEffect(() => {
    if (!isTauri()) return;
    invoke<AppSettings>("get_settings").then(setSettings).catch(() => setSaveState("error"));
  }, []);

  const save = async (next: AppSettings) => {
    setSettings(next);
    setSaveState("saving");
    try {
      await invoke("set_settings", { newSettings: next });
      setSaveState("saved");
    } catch (e) {
      setSaveState("error");
      setErrorMsg(String(e));
    }
  };

  const toggleLogin = async (enable: boolean) => {
    if (!settings) return;
    try {
      const registered = await invoke<boolean>("set_launch_at_login", { enable });
      setSettings({ ...settings, launch_at_login: registered });
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
  if (!settings) {
    return <div><h2>{t("Settings")}</h2><div className="card"><p className="note">{t("Loading settings…")}</p></div></div>;
  }

  return (
    <div>
      <h2>{t("Settings")}</h2>
      <div className="card">
        <h3>{t("Sampling")}</h3>
        <div className="info">
          <div className="label">{t("Foreground interval (ms)")}</div>
          <div className="value">
            <input
              type="number" min={500} max={10000} step={100}
              value={settings.foreground_interval_ms}
              onChange={(e) => save({ ...settings, foreground_interval_ms: Number(e.target.value) })}
              className="search-input" style={{ maxWidth: 120 }}
            />
          </div>
        </div>
        <div className="info">
          <div className="label">{t("Background interval (ms)")}</div>
          <div className="value">
            <input
              type="number" min={1000} max={30000} step={500}
              value={settings.background_interval_ms}
              onChange={(e) => save({ ...settings, background_interval_ms: Number(e.target.value) })}
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
              className={settings.launch_at_login ? "active" : ""}
              onClick={() => toggleLogin(!settings.launch_at_login)}
            >
              {settings.launch_at_login ? t("On") : t("Off")}
            </button>
          </div>
        </div>
        <p className="note">{t("Closing the window keeps monitoring in the menu bar; Quit stops collection.")}</p>
      </div>

      {saveState === "saved" && <p className="note">{t("Settings saved")}</p>}
      {saveState === "error" && <p className="note">{t("Failed to save settings")} {errorMsg}</p>}
    </div>
  );
}

export default App;