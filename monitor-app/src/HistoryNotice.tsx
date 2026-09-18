import type {HistoryView} from './history-view';
export function HistoryNotice({view,english=false}:{view?:HistoryView|null;english?:boolean}) {
  if(!view)return null;
  return <p className="note"><small>{view.recorded_boundaries?(english?'Recorded collection boundaries are separated; other gaps are inferred from intervals.':'按已记录的采集段分开显示，其余缺口按记录间隔推断。'):(english?'Continuity is inferred from recorded intervals.':'历史连续性按记录间隔推断。')}{english?' Aggregated records are shown separately.':' 汇总记录之间不连线。'}
    {view.aggregated&&(english?' Vertical ranges show recorded min–max; values may be averages.':' 竖线表示已记录的最小/最大范围，数值可能为均值。')}
    {view.downsampled&&(english?' Reduced display; only returned records are inspectable.':' 已抽稀显示，读数仅对应返回记录。')}
    {!!view.omitted_segments&&(english?` ${view.omitted_segments} separate sections omitted by the point limit.`:` 点数限制省略了 ${view.omitted_segments} 个独立数据段。`)}</small></p>;
}
