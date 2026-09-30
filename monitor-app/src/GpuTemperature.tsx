import {TemperatureTrend} from './TemperatureTrend';
import {useHistoryQuery} from './history-hooks';
import {sourceMessage} from './source-state';
import type {SourceState} from './source-state';
import type {GpuInfo} from './hardware';

export function GpuTemperature({gpu,historyId,state,english}:{gpu:GpuInfo;historyId:string;state?:SourceState;english:boolean}) {
  const history=useHistoryQuery('gpu.temperature.edge',historyId,3600);
  const reading=gpu.windows_temperature;
  const status=reading?.state??(state==='ok'?'warming_up':state??'warming_up');
  const value=reading?.edge_celsius;
  const valid=status==='ok'&&typeof value==='number'&&Number.isFinite(value);
  return <div className="card">
    <h3>{english?'GPU edge temperature':'GPU 边缘温度'}</h3>
    <div className="info"><span className="label">{english?'Current temperature':'当前温度'}</span><span className="value">{valid?`${value.toFixed(1)} °C`:'—'}</span></div>
    {!valid&&<p className="note" role="status">{sourceMessage(status,!english)}</p>}
    <p className="note">{english?'Average temperature at the edge of the GPU die. Read automatically with ordinary permissions.':'GPU 晶粒边缘的平均温度，普通权限下自动读取。'}</p>
    <TemperatureTrend history={history.points} view={history.view} status={history.status} label={`${gpu.name} ${english?'edge':'边缘'}`} english={english}/>
  </div>;
}
