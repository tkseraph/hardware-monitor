import {useState} from 'react';
import {invoke} from '@tauri-apps/api/core';
import type {ProcessDiskView} from './hardware';

export function ProcessDiskControls({value,english,onChanged}:{value:ProcessDiskView|null;english:boolean;onChanged:(value:ProcessDiskView)=>void}) {
  const [busy,setBusy]=useState(false);
  const [error,setError]=useState(false);
  const state=value?.state??'disabled';
  const active=['starting','warming_up','ready','incomplete','stale','stopping'].includes(state);
  const act=async(validation=false)=>{
    setBusy(true);setError(false);
    try {await invoke(active?'stop_process_disk':'start_process_disk',{validation});onChanged(await invoke<ProcessDiskView>('get_process_disk'));}
    catch {setError(true);}finally {setBusy(false);}
  };
  const messages:Record<string,[string,string]>={
    disabled:['尚未启用进程磁盘读写采集。','Process disk collection is disabled.'],
    starting:['等待管理员授权并连接采集组件…','Waiting for administrator approval and connection…'],
    warming_up:['正在建立进程身份和采样窗口…','Verifying process identities and warming up…'],
    ready:['当前窗口可用，仅显示身份已核验且覆盖完整窗口的进程。','The current window is usable for verified process instances present throughout it.'],
    incomplete:['当前窗口不完整，读取和写入速率暂不显示。','The current window is incomplete; read and write rates are unavailable.'],
    stale:['采集数据已过期，等待新窗口。','Readings are stale; waiting for a new window.'],
    stopping:['正在停止采集并释放会话…','Stopping collection and releasing the session…'],
    error:['采集未能启动或已结束，速率已清空。','Collection failed or ended; rates have been cleared.'],
  };
  const reasons:Record<string,[string,string]>={
    permission_required:['需要管理员授权；授权取消后可再次启用。','Administrator approval is required; retry if approval was cancelled.'],
    clock:['采样时钟发生变化，请重新启用采集。','The sample clock changed; enable collection again.'],
    loss:['发现事件丢失，请重新启用采集。','Events were lost; enable collection again.'],
    pipeline:['采样顺序或身份校验中断，请重新启用采集。','Event ordering or identity verification failed; enable collection again.'],
    baseline:['无法建立有效的进程基线。','A verified process baseline could not be established.'],
    start_failed:['磁盘事件采集未能启动。','Disk event collection could not start.'],
    open_trace:['磁盘事件接收未能启动。','The disk event consumer could not start.'],
    source_ended:['采集连接已结束。','The collection connection ended.'],
    cleanup_unconfirmed:['尚未确认采集会话已释放，暂不重复启动。','Session cleanup is unconfirmed; another session cannot start yet.'],
  };
  const text=(pair:[string,string])=>pair[english?1:0];
  const quality=value?.quality;
  return <article className="card">
    <div className="disk-summary"><h3>{english?'Process disk I/O':'进程磁盘读写'}</h3><button disabled={busy||state==='stopping'||value?.reason==='cleanup_unconfirmed'} onClick={()=>void act()}>{active?(english?'Stop disk collection':'停止磁盘采集'):(english?'Enable disk collection':'启用磁盘采集')}</button></div>
    <p className="note">{english?'Enabling needs administrator approval. Rates cover physical disk events across all disks and the verified subset of processes. They are not lifetime totals or all process I/O. No process paths or disk history are recorded.':'启用时需确认管理员提示。速率来自所有磁盘的物理读写事件，仅覆盖已核验的部分进程，不代表程序的全部文件读写或累计读写量。不记录进程路径或磁盘事件历史。'}</p>
    <p role="status">{text(messages[state]??messages.error)}</p>
    {value?.reason&&value.reason!=='none'&&reasons[value.reason]&&<p role="status">{text(reasons[value.reason])}</p>}
    {error&&<p role="alert">{english?'Could not change collection; check the administrator prompt.':'采集切换失败，请检查管理员提示。'}</p>}
    {value?.window_ms!=null&&<p className="note">{english?`Sample window: ${value.window_ms} ms · ${value.known_processes} process instances with usable rates`:`采样窗口 ${value.window_ms} 毫秒 · ${value.known_processes} 个进程实例具有有效速率`}</p>}
    {!!quality?.unattributed_operations&&<p className="note">{english?`${quality.unattributed_operations} operations could not be attributed in this window (${quality.unattributed_bytes} bytes).`:`本窗口有 ${quality.unattributed_operations} 次读写无法确认归属（${quality.unattributed_bytes} 字节）。`}</p>}
    {!!value?.identities.unavailable&&<details><summary>{english?'Identity verification details':'身份核验详情'}</summary><p className="note">{english?`Since enabling: ${value.identities.resolved} initiation identities verified; ${value.identities.unavailable} unavailable. Unavailable: ${value.identities.open_failed} handles or identity reads, ${value.identities.lifetime_unverified} event-time lifetimes, ${value.identities.clock_unverified} clock conversions, ${value.identities.capacity_rejected} capacity limits.`:`本次启用以来：${value.identities.resolved} 次发起身份已核验，${value.identities.unavailable} 次不可确认。其中句柄或身份读取失败 ${value.identities.open_failed} 次、事件时间与生命周期不符 ${value.identities.lifetime_unverified} 次、时钟换算失败 ${value.identities.clock_unverified} 次、容量受限 ${value.identities.capacity_rejected} 次。`}</p></details>}
    {!active&&<button disabled={busy||value?.reason==='cleanup_unconfirmed'} onClick={()=>void act(true)}>{english?'Verify once (stops after 20 seconds)':'单次验证（采集 20 秒自动停止）'}</button>}
    <p className="note">{english?'Stop collection or quit the app to end this session. Closing the window keeps it running in the tray.':'停止采集或退出软件后结束本次会话；关闭窗口后仍在托盘采集。'}</p>
  </article>;
}
