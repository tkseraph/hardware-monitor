import {TemperatureTrend} from "./TemperatureTrend";
import {EnhancedStorage} from "./EnhancedStorage";
import { HistoryNotice } from "./HistoryNotice";
import { useState } from 'react';
import { orderOverviewDisks } from './storage-order';
import { useHistoryQuery } from './history-hooks';
import { temperatureSegments } from './history-data';
import { sourceMessage } from './source-state';
import { groupVolumes, volumeUsage, rateForDisk } from './windows-storage';
import type { WindowsStorageSnapshot, WindowsThroughput, WindowsVolume } from './windows-storage';
import './windows-storage.css';

function bytes(value: number | null): string {
  if (value === null) return '—';
  if (value >= 1024 ** 4) return `${(value/1024**4).toFixed(2)} TiB`;
  return `${(value/1024**3).toFixed(2)} GiB`;
}
function rate(value: number | null | undefined): string {
  if (value === null || value === undefined) return '—';
  if (value >= 1e6) return `${(value/1e6).toFixed(2)} MB/s`;
  if (value >= 1e3) return `${(value/1e3).toFixed(1)} kB/s`;
  return `${value.toFixed(0)} B/s`;
}
function Volumes({volumes, english}: {volumes: WindowsVolume[]; english: boolean}) {
  return <table className="volume-table"><thead><tr><th>{english?'Volume':'卷'}</th><th>{english?'Capacity':'容量'}</th><th>{english?'Used':'已用'}</th><th>{english?'Free':'空闲'}</th></tr></thead><tbody>{volumes.map(v => {
    const usage=volumeUsage(v);
    return <tr key={v.alias}><th>{v.drive_letter ? `${v.drive_letter}:` : v.alias}<small>{!v.drive_letter && (english?' · no drive letter':' · 无盘符')}</small></th><td>{bytes(v.size_bytes)}</td><td>{bytes(usage?.used??null)}{usage && <small> · {usage.percent.toFixed(1)}%</small>}</td><td>{bytes(v.free_bytes)}</td></tr>;
  })}</tbody></table>;
}
function ThroughputHistory({uid, english, intervalMs}: {uid:string;english:boolean;intervalMs:number}) {
  const {points,status,loaded,view,segments:wireSegments}=useHistoryQuery('disk.throughput',uid,3600);
  const valid=points.filter(([t,v])=>Number.isFinite(t)&&Number.isFinite(v)&&v>=0);
  const segments=view?wireSegments:temperatureSegments(valid,Math.max(15,intervalMs/1000*3),200);
  const bounds=view?.segments.flat()??[];
  const min=valid[0]?.[0]??0,max=valid[valid.length-1]?.[0]??1;
  const peak=Math.max(1,...(bounds.length?bounds.map(p=>p.max):valid.map(([,v])=>v)));
  return <div className="disk-history"><p>{english?'Read + write · last hour · MB/s':'读写合计 · 最近一小时 · MB/s'}</p>
    <HistoryNotice view={view} english={english}/>
    {status==='error' ? <p>{english?'History could not be loaded':'历史读取失败'}</p> : valid.length ? <>
      <small>{english?'Chart maximum':'纵轴上限'} {peak.toFixed(2)} MB/s</small>
      <svg viewBox="0 0 600 110" role="img" aria-label={english?'Disk throughput history':'磁盘吞吐历史曲线'}>{bounds.filter(p=>p.min!==p.max).map((p,i)=><line key={`range-${i}`} x1={(p.t-min)/Math.max(1,max-min)*600} x2={(p.t-min)/Math.max(1,max-min)*600} y1={100-p.min/peak*95} y2={100-p.max/peak*95} stroke="#60a5fa"/>)}{segments.map((segment,i)=>segment.length===1?<circle key={i} cx={(segment[0][0]-min)/Math.max(1,max-min)*600} cy={100-segment[0][1]/peak*95} r="2" fill="#60a5fa"/>:<polyline key={i} points={segment.map(([t,v])=>`${(t-min)/Math.max(1,max-min)*600},${100-v/peak*95}`).join(' ')} />)}</svg>
      <div className="disk-summary"><small>{new Date(min*1000).toLocaleTimeString()}</small><small>{new Date(max*1000).toLocaleTimeString()}</small></div>
    </> : <p>{loaded?(english?'No history for this connection yet':'本次连接暂无历史'):(english?'Loading history…':'正在加载历史…')}</p>}
  </div>;
}
function TemperatureHistory({uid,label,english}:{uid:string;label:string;english:boolean}){
  const {points,view,status}=useHistoryQuery('disk.temperature',uid,3600);
  return <TemperatureTrend history={points} view={view} status={status} label={label} english={english}/>;
}
export function WindowsStorage({snapshot,throughput,english=false,details=false,intervalMs=1000,onDetails}: {
  snapshot: WindowsStorageSnapshot;throughput?:WindowsThroughput|null;english?:boolean;details?:boolean;intervalMs?:number;onDetails?:()=>void;
}) {
  const [archive,setArchive]=useState('');
  const groups=groupVolumes(snapshot.disks,snapshot.volumes);
  return <section className="windows-storage">
    <div className="disk-summary"><h2>{english?'Storage devices':'存储设备'}</h2>{!details&&onDetails&&<button onClick={onDetails}>{english?'Storage details':'存储详情'}</button>}</div>
    {details&&<EnhancedStorage english={english}/>}
    {snapshot.registry_state!=='ok'&&<p className="note">{english?'Device history registry is unavailable; new device history is not associated.':'设备历史注册表暂不可用，未确认身份的磁盘不记录新历史。'}</p>}
    {snapshot.volume_state!=='ok'&&<p className="note">{english?'Volume capacity could not be read.':'卷容量读取失败。'} {sourceMessage(snapshot.volume_state,!english)}</p>}
    {orderOverviewDisks(snapshot.disks).map(d=>{
      const perf=rateForDisk(d,throughput);
      return <article className="card" key={d.device_uid??`disk-${d.number}`}>
        <div className="disk-summary"><h3>{d.name}</h3><span>{d.kind==='physical'?(english?'Physical disk':'物理盘'):(english?'System disk device':'系统磁盘设备')} {d.number} · {d.bus_type===17?'NVMe':`Bus ${d.bus_type}`}</span></div>
        <div className="disk-summary"><span>{english?'Device capacity':'设备容量'}：{bytes(d.size_bytes)}</span><span>{english?'Read':'读'}：{rate(perf?.read_bps)}　{english?'Write':'写'}：{rate(perf?.write_bps)}</span></div>
        {d.kind!=='physical'&&<p className="note">{english?'Underlying physical devices have not been verified.':'尚未验证底层物理设备，不计为已确认的独立物理盘。'}</p>}
        {d.identity_state!=='ok'&&<p className="note">{english?'Device identity is unverified; performance and history are not associated.':'设备身份尚未验证，暂不关联性能与历史。'}</p>}
        {perf&&perf.state!=='ok'&&<p>{sourceMessage(perf.state,!english)}</p>}
        {groups.dedicated[d.number]?.length ? <Volumes volumes={groups.dedicated[d.number]} english={english}/> : <p>{english?'No uniquely associated volume':'暂无可单独归属的卷'}</p>}
        {details&&d.device_uid&&<><ThroughputHistory uid={d.device_uid} english={english} intervalMs={intervalMs}/><TemperatureHistory uid={d.device_uid} label={d.name} english={english}/></>}
      </article>;
    })}
    {!!groups.shared.length&&<article className="card"><h3>{english?'Shared or unassigned volumes':'跨盘或未确认归属的卷'}</h3><p>{english?'Capacity is shown once here and is not assigned to a single physical disk.':'容量只在此处展示一次，不归算到某一块物理盘。'}</p><Volumes volumes={groups.shared} english={english}/></article>}
    {!!throughput?.unmapped_instances&&<p className="note">{english?`${throughput.unmapped_instances} performance instances have no confirmed device mapping.`:`有 ${throughput.unmapped_instances} 个性能实例尚未确认设备映射。`}</p>}
    {details&&<article className="card"><h3>{english?'Saved connection histories':'已保存的连接历史'}</h3><p>{english?'A restart starts a new series when continuity cannot be confirmed. Old records remain available; matching descriptions do not prove the same device.':'无法确认连续性时，重启会建立新序列。旧记录仍可查看；名称与容量相同不代表已确认是同一设备。'}</p>
      <select aria-label={english?'Historical connection':'历史连接'} value={archive} onChange={e=>setArchive(e.target.value)}><option value="">{english?'Choose a saved series':'选择历史序列'}</option>{snapshot.history_series.map(s=><option key={s.uid} value={s.uid}>{s.name} · {new Date(s.created_at*1000).toLocaleString()} · {s.uid.slice(0,8)}</option>)}</select>
      {archive&&<><ThroughputHistory uid={archive} english={english} intervalMs={intervalMs}/><TemperatureHistory uid={archive} label={english?"Saved disk":"历史磁盘"} english={english}/></>}</article>}
  </section>;
}
