import { HistoryNotice } from "./HistoryNotice";
import type { HistoryView } from "./history-view";
import { useState } from "react";
import { temperatureSegments, gapThresholdSecs } from "./history-data";
import type { HistoryStatus } from "./history-hooks";
import "./temperature-trend.css";

type Point = [number, number];
export function TemperatureTrend({ history, view, status, label, english = false, end = Math.floor(Date.now() / 1000) }: {
  history: Point[]; view?:HistoryView|null; status: HistoryStatus; label: string; english?: boolean; end?: number;
}) {
  const [selected, setSelected] = useState<number | null>(null);
  const start = end - 3600;
  const points = history.filter(([t, v]) => Number.isFinite(t) && Number.isFinite(v) && t >= start && t <= end);
  const bounds=view?.segments.flat().filter(p=>p.t>=start&&p.t<=end)??[];
  const values = bounds.length?bounds.flatMap(p=>[p.min,p.max]):points.map(([, v]) => v);
  // At least 20°C of vertical range: tiny changes must not look dramatic.
  const low = values.length ? Math.floor(Math.min(...values) / 10) * 10 - 10 : 0;
  const high = values.length ? Math.max(low + 20, Math.ceil(Math.max(...values) / 10) * 10 + 10) : 20;
  const x = (t: number) => (t - start) / 3600 * 1000;
  const y = (v: number) => 10 + (high - v) / (high - low) * 100;
  const segments:Point[][] = view ? view.segments.map(s=>s.filter(p=>p.t>=start&&p.t<=end).map(p=>[p.t,p.value] as Point)).filter(s=>s.length) : temperatureSegments(points, gapThresholdSecs(3600));
  const active = selected === null ? undefined : points.find(([t]) => t === selected);
  const describe = ([t, v]: Point) => `${new Date(t * 1000).toLocaleTimeString(english ? "en-GB" : "zh-CN", { hour12: false })} · ${v.toFixed(1)} °C`;
  const heading = english ? "Temperature trend" : "温度趋势";
  return <section className="temperature-trend" aria-label={`${label} ${heading}`}>
    <header><strong><i aria-hidden="true" />{heading}</strong><span>{english ? "Last hour" : "最近 1 小时"}</span></header>
    {status === "error" || !points.length ? <p className="temperature-empty" role="status">{
      status === "error" ? (english ? "Temperature history could not be loaded" : "温度历史加载失败") :
      status === "loading" ? (english ? "Loading temperature history…" : "正在加载温度历史…") :
      (english ? "No temperature history yet" : "暂无温度历史")
    }</p> : <>
      <HistoryNotice view={view} english={english}/>
      <div className="temperature-readout" aria-live="polite">{active ? describe(active) : english ? "Hover, tap or use ← → to inspect" : "悬停、轻点或按 ← → 查看温度"}</div>
      <div className="temperature-plot">
        <div className="temperature-axis" aria-hidden="true">{[high, (high + low) / 2, low].map(v => <span key={v}>{v} °C</span>)}</div>
        <svg viewBox="0 0 1000 120" preserveAspectRatio="none" tabIndex={0} role="img"
          aria-label={`${label} ${heading}, ${english ? 'last hour; degrees Celsius; arrow keys select samples' : '最近一小时，单位摄氏度；方向键选择采样点'}`}
          onPointerMove={e => {
            const box = e.currentTarget.getBoundingClientRect();
            const time = start + (e.clientX - box.left) / box.width * 3600;
            const segment=segments.find(s=>s.length>1&&time>=s[0][0]&&time<=s[s.length-1][0]);
            const candidates=segment??points.filter(([t])=>Math.abs(t-time)<=1);
            const nearest=candidates.length?candidates.reduce((a,b)=>Math.abs(a[0]-time)<Math.abs(b[0]-time)?a:b):undefined;
            setSelected(nearest?.[0]??null);
          }} onPointerLeave={() => setSelected(null)} onBlur={() => setSelected(null)}
          onKeyDown={e => {
            if (e.key === 'Escape') { setSelected(null); return; }
            if (e.key !== 'ArrowLeft' && e.key !== 'ArrowRight') return;
            e.preventDefault();
            const index = points.findIndex(([t]) => t === selected);
            setSelected(points[index < 0 ? points.length - 1 : Math.max(0, Math.min(points.length - 1, index + (e.key === 'ArrowLeft' ? -1 : 1)))][0]);
          }}>
          {[10, 60, 110].map(pos => <line className="temperature-grid" key={pos} x1="0" x2="1000" y1={pos} y2={pos} vectorEffect="non-scaling-stroke" />)}
          {bounds.filter(p=>p.min!==p.max).map((p,i)=><line key={`range-${i}`} className="temperature-line" x1={x(p.t)} x2={x(p.t)} y1={y(p.min)} y2={y(p.max)}><title>{`${p.count} records · ${p.min}–${p.max} °C`}</title></line>)}
          {segments.map((seg, i) => seg.length === 1 ? <path key={i} className="temperature-line" d={`M${x(seg[0][0])},${y(seg[0][1])}h0.1`} /> :
            <path key={i} className="temperature-line" d={seg.map(([t, v], j) => `${j ? 'L' : 'M'}${x(t)},${y(v)}`).join(' ')} />)}
          {active && <line className="temperature-cursor" x1={x(active[0])} x2={x(active[0])} y1="0" y2="120" vectorEffect="non-scaling-stroke" />}
        </svg>
      </div>
      <div className="temperature-time"><span>{english ? '1 hour ago' : '1 小时前'}</span><span>{english ? '30 min ago' : '30 分钟前'}</span><span>{english ? 'Now' : '现在'}</span></div>
      <footer><span>{english ? 'Recorded range' : '已记录范围'} {Math.min(...values).toFixed(1)}–{Math.max(...values).toFixed(1)} °C</span><span>{english ? 'Gaps mean missing data' : '空白表示无采样数据'}</span></footer>
    </>}
  </section>;
}
