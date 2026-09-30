import {useEffect,useState} from 'react';
import {invoke,isTauri} from '@tauri-apps/api/core';
import {TemperatureTrend} from './TemperatureTrend';
import {useHistoryQuery} from './history-hooks';
type Series={uid:string;name:string;sensor:string;created_at:number};
type Status={state:string;observed_at:number;celsius:number|null;reason:string;history_uid:string|null;history_series:Series[];history_lost:number};
function SavedTrend({uid,english}:{uid:string;english:boolean}){
  const history=useHistoryQuery('cpu.temperature.tctl',uid,3600);
  return <TemperatureTrend history={history.points} view={history.view} status={history.status} label={english?'CPU control temperature':'CPU 控制温度'} english={english}/>;
}
export function CpuTemperature({english}:{english:boolean}){
  const [value,setValue]=useState<Status>({state:'disabled',observed_at:0,celsius:null,reason:'',history_uid:null,history_series:[],history_lost:0});
  const [busy,setBusy]=useState(false);const [error,setError]=useState(false);const [archive,setArchive]=useState('');
  useEffect(()=>{
    if(!isTauri())return;
    let disposed=false,inFlight=false;
    const refresh=async()=>{if(inFlight)return;inFlight=true;try{const next=await invoke<Status>('get_cpu_enhanced');if(!disposed){setValue(next);setError(false);}}catch{if(!disposed)setError(true);}finally{inFlight=false;}};
    void refresh();const timer=setInterval(()=>void refresh(),1000);
    return()=>{disposed=true;clearInterval(timer);};
  },[]);
  const active=['starting','running','read_error','stopping'].includes(value.state);
  const act=async()=>{setBusy(true);setError(false);try{await invoke(active?'stop_cpu_enhanced':'start_cpu_enhanced');setValue(await invoke<Status>('get_cpu_enhanced'));}catch{setError(true);}finally{setBusy(false);}};
  const stale=value.observed_at>0&&Date.now()/1000-value.observed_at>15;
  const valid=value.state==='running'&&!stale&&value.celsius!==null&&Number.isFinite(value.celsius);
  const uid=archive||value.history_uid;
  const reasons:Record<string,[string,string]>={component_missing:['CPU 温度组件尚未安装。','The CPU temperature component is not installed.'],permission_required:['读取 CPU 温度需要管理员授权。','CPU temperature needs administrator approval.'],driver_unverified:['已安装的 CPU 温度组件尚未核验。','The installed CPU temperature component has not been verified.'],driver_version_unverified:['CPU 温度组件版本尚未核验。','The CPU temperature component version has not been verified.'],identity_unverified:['CPU 身份尚未确认，暂不记录温度。','CPU identity has not been confirmed; temperature is not recorded.'],unsupported_cpu:['当前 CPU 型号尚未验证。','This CPU model has not been verified.']};
  const message=reasons[value.reason]?.[english?1:0]??(english?'The latest temperature could not be read.':'本次温度读取失败。');
  return <article className="card">
    <div className="disk-summary"><h3>{english?'CPU control temperature (Tctl)':'CPU 控制温度（Tctl）'}</h3><button disabled={busy||value.state==='stopping'} onClick={()=>void act()}>{active?(english?'Stop temperature collection':'停止温度采集'):(english?'Enable temperature collection':'启用温度采集')}</button></div>
    <p className="note">{english?'Enabling requests administrator approval for CPU temperature collection. Stopping collection or quitting ends the companion; the installed PawnIO driver remains.':'启用时需确认管理员提示。停止采集或退出软件后结束采集组件；已安装的 PawnIO 驱动保留。'}</p>
    <div className="info"><span className="label">{english?'Current temperature':'当前温度'}</span><span className="value">{valid?`${value.celsius!.toFixed(1)} °C`:'—'}</span></div>
    <p className="note">{english?'CPU-reported control temperature, in Celsius.':'CPU 报告的控制温度，单位为摄氏度。'}</p>
    {value.state==='starting'&&<p role="status">{english?'Waiting for approval and connection…':'等待授权并连接采集组件…'}</p>}
    {value.state==='stopping'&&<p role="status">{english?'Stopping collection…':'正在停止采集…'}</p>}
    {error&&<p role="alert">{english?'Collection status could not be loaded or changed.':'采集状态读取或切换失败。'}</p>}
    {value.state==='error'&&<p role="alert">{english?'Collection could not start or lost its connection.':'采集未能启动或连接已结束。'}</p>}
    {value.state==='read_error'&&<p role="status">{message}</p>}
    {stale&&<p role="status">{english?'Readings are stale; waiting for fresh data.':'读数已过期，等待新的有效数据。'}</p>}
    {!!value.history_lost&&<p role="status">{english?`${value.history_lost} history samples were not saved.`:`有 ${value.history_lost} 个温度样本未保存。`}</p>}
    {valid&&<small>{english?'Updated':'更新时间'}：{new Date(value.observed_at*1000).toLocaleTimeString()}</small>}
    {value.history_series.length>0&&<><p className="note">{english?'A restart creates a new series; previously saved records remain available.':'软件重新启动会建立新序列，已保存记录仍可查看。'}</p><select aria-label={english?'CPU temperature history':'CPU 温度历史'} value={archive} onChange={e=>setArchive(e.target.value)}><option value="">{english?'Current startup':'本次启动'}</option>{value.history_series.filter(s=>s.uid!==value.history_uid).map(s=><option key={s.uid} value={s.uid}>{s.name} · {new Date(s.created_at*1000).toLocaleString()} · {s.uid.slice(0,8)}</option>)}</select></>}
    {uid?<SavedTrend key={uid} uid={uid} english={english}/>:<p>{english?'Enable collection to start recording temperature history.':'启用采集后开始记录温度历史。'}</p>}
  </article>;
}
