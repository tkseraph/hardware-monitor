import {useEffect,useState} from 'react';
import {invoke,isTauri} from '@tauri-apps/api/core';
type Disk={number:number;name:string;temperature_c:number|null;health:number|null;wear_used_percent:number|null;power_on_hours:number|null;state:string;series_uid:string|null};
type Status={state:string;observed_at:number;disks:Disk[];history_lost:number};
export function EnhancedStorage({english}:{english:boolean}){
  const [value,setValue]=useState<Status>({state:'disabled',observed_at:0,disks:[],history_lost:0});
  const [busy,setBusy]=useState(false);const [error,setError]=useState(false);
  useEffect(()=>{
    if(!isTauri())return;
    let disposed=false,inFlight=false;
    const refresh=async()=>{if(inFlight)return;inFlight=true;try{const next=await invoke<Status>('get_storage_enhanced');if(!disposed)setValue(next);}catch{if(!disposed)setError(true);}finally{inFlight=false;}};
    void refresh();const timer=setInterval(()=>void refresh(),1000);
    return()=>{disposed=true;clearInterval(timer);};
  },[]);
  const act=async(command:string)=>{setBusy(true);setError(false);try{await invoke(command);setValue(await invoke<Status>('get_storage_enhanced'));}catch{setError(true);}finally{setBusy(false);}};
  const active=['starting','running','read_error','stopping'].includes(value.state);
  const stale=value.observed_at>0&&Date.now()/1000-value.observed_at>20;
  const health=(n:number|null)=>n===0?(english?'Normal':'正常'):n===1?(english?'Warning':'警告'):n===2?(english?'Unhealthy':'异常'):(english?'Unknown':'未知');
  return <article className="card">
    <div className="disk-summary"><h3>{english?'Disk temperature and health':'磁盘温度与健康'}</h3>
      <button disabled={busy||value.state==='stopping'} onClick={()=>void act(active?'stop_storage_enhanced':'start_storage_enhanced')}>
        {active?(english?'Stop enhanced collection':'停止增强采集'):(english?'Enable enhanced collection':'启用增强采集')}
      </button>
    </div>
    <p>{english?'Enabling requests Windows administrator approval for a storage-only companion. It stops when you stop collection or quit the app. No driver is installed.':'启用时需确认 Windows 管理员提示，仅启动磁盘只读采集组件。停止采集或退出软件后结束，不安装驱动。'}</p>
    {value.state==='starting'&&<p role="status">{english?'Waiting for approval and connection…':'等待授权并连接采集组件…'}</p>}
    {value.state==='stopping'&&<p role="status">{english?'Stopping collection…':'正在停止采集…'}</p>}
    {(error||value.state==='error'||value.state==='read_error')&&<p role="alert">{english?'Collection could not start or read data. Authorization may have been cancelled; you can retry.':'采集未能启动或读取失败，可能是授权被取消，可重试。'}</p>}
    {!!value.history_lost&&<p role="status">{english?`${value.history_lost} temperature history saves were missed; live readings continue.`:`有 ${value.history_lost} 次温度历史未保存，实时读数继续更新。`}</p>}
    {stale&&<p role="status">{english?'Readings are stale; waiting for fresh data.':'读数已过期，等待新的有效数据。'}</p>}
    {value.state==='running'&&!stale&&<>
      <p><small>{english?'Updated':'更新时间'}：{new Date(value.observed_at*1000).toLocaleTimeString()}</small></p>
      <table className="volume-table"><thead><tr><th>{english?'Device':'设备'}</th><th>{english?'Temperature':'温度'}</th><th>{english?'Windows status':'Windows 状态'}</th><th>{english?'Wear used':'已用磨损'}</th><th>{english?'Power-on hours':'通电小时'}</th></tr></thead>
      <tbody>{value.disks.map(d=><tr key={d.number}><th>{d.name}<small> · {d.number}</small></th><td>{d.temperature_c===null?'—':`${d.temperature_c} °C`}</td><td>{health(d.health)}</td><td>{d.wear_used_percent===null?'—':`${d.wear_used_percent}%`}</td><td>{d.power_on_hours??'—'}</td></tr>)}</tbody></table>
      {!value.disks.length&&<p>{english?'No disk readings returned.':'本次未返回磁盘读数。'}</p>}
    </>}
    <p><small>{english?'Unknown readings show —. Windows status is not remaining lifespan; wear is not remaining health. Temperature history is recorded only for verified device connections.':'未提供的读数显示 —。Windows 状态不代表剩余寿命，磨损值也不是健康百分比。仅对身份已确认的设备记录温度历史。'}</small></p>
  </article>;
}
