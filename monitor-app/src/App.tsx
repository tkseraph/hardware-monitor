import { createContext, useContext, useEffect, useState } from "react";
import { invoke, isTauri } from "@tauri-apps/api/core";

type Language = "zh" | "en";
const LanguageContext = createContext<Language>("zh");
const translations: Record<string, string> = {"System Overview": "系统总览", "CPU Details": "处理器详情", "Memory Details": "内存详情", "GPU Details": "图形处理器详情", "Disk Details": "磁盘详情", "Process Ranking": "进程排行", "Settings": "设置", "No matching processes": "暂无匹配的进程", "General": "通用", "CPU": "处理器", "GPU": "图形处理器", "Memory": "内存", "Disks": "存储设备", "Name:": "名称", "Cores:": "核心数量", "Usage:": "使用率", "Total:": "总容量", "Used:": "已使用", "Utilization:": "利用率", "Memory:": "内存用量", "Active I/O:": "有吞吐的设备", "Physical Cores:": "物理核心", "Logical Processors:": "逻辑处理器", "Total Usage:": "总使用率", "Per-Core Usage": "逐核使用率", "Usage History (Last Hour)": "使用率历史 · 最近一小时", "System Memory": "系统内存", "Available:": "可用", "In Use:": "使用中", "Allocated:": "已分配", "Unified memory architecture - no separate VRAM": "统一内存架构，无独立显存；以下为驱动统计，不代表独立显存容量。", "Device:": "设备标识", "Capacity:": "容量", "SMART Status:": "SMART 摘要", "Throughput:": "合计吞吐", "Name": "进程名称", "Sort by Memory": "按内存排序", "Sort by CPU": "按 CPU 排序", "Settings will be implemented in a future update.": "采样频率、历史保留与登录项设置尚未实现。"};
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
  name: string;
  memory_bytes: number;
  cpu_usage: number;
}

interface SystemInfo {
  cpu: CpuInfo;
  memory: MemoryInfo;
  gpu: GpuInfo;
  disks: DiskInfo[];
  disk_throughput: DiskThroughput[];
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

function Chart({ history, label }: { history: [number, number][]; label: string }) {
  if (history.length === 0) return null;
  const points = history.map(([_ts, val], idx) => {
    const x = history.length === 1 ? 0 : (idx / (history.length - 1)) * 800;
    const y = 210 - (Math.max(0, Math.min(100, val)) / 100) * 170;
    return [x, y] as const;
  });
  const line = points.map(([x, y], i) => `${i === 0 ? "M" : "L"}${x},${y}`).join(" ");
  const area = `${line} L800,220 L0,220 Z`;
  return (
    <div className="chart">
      <svg width="100%" height="220" viewBox="0 0 800 220" preserveAspectRatio="none">
        <defs>
          <linearGradient id={`${label}-area`} x1="0" x2="0" y1="0" y2="1">
            <stop offset="0%" stopColor="rgba(10,132,255,.28)" />
            <stop offset="100%" stopColor="rgba(10,132,255,.04)" />
          </linearGradient>
        </defs>
        <g stroke="rgba(148,163,184,.18)" strokeWidth="1">
          {[40, 80, 120, 160].map((y) => (
            <line key={y} x1="0" y1={y} x2="800" y2={y} />
          ))}
        </g>
        <path d={area} fill={`url(#${label}-area)`} stroke="none" />
        <path d={line} fill="none" stroke="#0a84ff" strokeWidth="2.5" strokeLinejoin="round" strokeLinecap="round" />
      </svg>
      <div className="chart-labels">
        <span>0%</span>
        <span>100%</span>
      </div>
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

  useEffect(() => {
    const fetchHistory = async () => {
      try {
        const data = await invoke<[number, number][]>("get_history", {
          metricId: "disk.throughput",
          objectId: "disk0",
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

      {history.length > 0 && (
        <div className="card">
          <h3>{t("Usage History (Last Hour)")}</h3>
          <Chart history={history} label="disk" />
        </div>
      )}
    </div>
  );
}

function ProcessesPage() {
  const t = useText();
  const [processes, setProcesses] = useState<ProcessInfo[]>([]);
  const [sortBy, setSortBy] = useState<"memory" | "cpu">("memory");
  const [searchTerm, setSearchTerm] = useState("");

  useEffect(() => {
    const fetchProcesses = async () => {
      try {
        const procs = await invoke<ProcessInfo[]>("get_processes");
        setProcesses(procs);
      } catch (err) {
        console.error("Failed to fetch processes:", err);
      }
    };

    fetchProcesses();
    const interval = setInterval(fetchProcesses, 2000);
    return () => clearInterval(interval);
  }, []);

  const sortedProcesses = [...processes]
    .filter((p) => p.name.toLowerCase().includes(searchTerm.toLowerCase()))
    .sort((a, b) => {
      if (sortBy === "memory") {
        return b.memory_bytes - a.memory_bytes;
      } else {
        return b.cpu_usage - a.cpu_usage;
      }
    });

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
            <button
              className={sortBy === "memory" ? "active" : ""}
              onClick={() => setSortBy("memory")}
            >
              {t("Sort by Memory")}
            </button>
            <button
              className={sortBy === "cpu" ? "active" : ""}
              onClick={() => setSortBy("cpu")}
            >
              {t("Sort by CPU")}
            </button>
          </div>
        </div>
        <table className="process-table">
          <thead>
            <tr>
              <th>PID</th>
              <th>{t("Name")}</th>
              <th>{t("Memory")}</th>
              <th>CPU %</th>
            </tr>
          </thead>
          <tbody>
            {sortedProcesses.length === 0 && <tr><td colSpan={4}>{t("No matching processes")}</td></tr>}
            {sortedProcesses.map((proc) => (
              <tr key={proc.pid}>
                <td>{proc.pid}</td>
                <td>{proc.name}</td>
                <td>{formatBytes(proc.memory_bytes)}</td>
                <td>{proc.cpu_usage.toFixed(1)}%</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}

function SettingsPage() {
  const t = useText();
  return (
    <div>
      <h2>{t("Settings")}</h2>
      <div className="card">
        <h3>{t("General")}</h3>
        <p className="note">{t("Settings will be implemented in a future update.")}</p>
      </div>
    </div>
  );
}

export default App;