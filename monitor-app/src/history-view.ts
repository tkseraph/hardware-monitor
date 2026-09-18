export interface HistoryPoint {
  t:number;value:number;min:number;max:number;count:number;granularity_secs:number;first_ts:number;last_ts:number;
}
export interface HistoryView {
  version:2;recorded_boundaries?:boolean;segments:HistoryPoint[][];input_points:number;downsampled:boolean;
  omitted_segments:number;aggregated:boolean;continuity:string;
}
export function historySegments(view:HistoryView): [number,number][][] {
  if(view.version!==2||!Array.isArray(view.segments))throw new Error('Unsupported history response');
  let total=0;
  return view.segments.map(segment=>segment.map((point,index)=>{
    if(++total>2000||![point.t,point.value,point.min,point.max].every(Number.isFinite)||point.min>point.max||(index>0&&point.t<segment[index-1].t))throw new Error('Invalid history response');
    return [point.t,point.value];
  }));
}
