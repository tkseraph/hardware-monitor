// Versioned segmented history. A changed key never exposes the previous query's data.
import {useEffect,useMemo,useState} from "react";
import {invoke,isTauri} from "@tauri-apps/api/core";
import {historySegments,type HistoryView} from "./history-view";
export type HistoryStatus="idle"|"loading"|"ok"|"error";
export interface HistoryQuery {points:[number,number][];segments:[number,number][][];view:HistoryView|null;status:HistoryStatus;loaded:boolean}
export function useHistoryQuery(metricId:string,objectId:string,durationSecs:number,pollMs=5000):HistoryQuery {
  const key=JSON.stringify([metricId,objectId,durationSecs]);
  const [state,setState]=useState<{key:string;view:HistoryView|null;status:HistoryStatus;loaded:boolean}>({key:"",view:null,status:"idle",loaded:false});
  useEffect(()=>{
    if(!isTauri()||!objectId){setState({key,view:null,status:"idle",loaded:false});return;}
    let cancelled=false,busy=false;
    setState({key,view:null,status:"loading",loaded:false});
    const fetchOnce=async()=>{
      if(cancelled||busy||document.visibilityState==="hidden")return;
      busy=true;
      try {
        const view=await invoke<HistoryView>("get_history_v2",{metricId,objectId,durationSecs});
        historySegments(view);
        if(!cancelled)setState({key,view,status:"ok",loaded:true});
      }catch{if(!cancelled)setState({key,view:null,status:"error",loaded:true});}
      finally{busy=false;}
    };
    void fetchOnce();const timer=setInterval(()=>void fetchOnce(),pollMs);
    const visible=()=>{if(document.visibilityState==="visible")void fetchOnce();};
    document.addEventListener("visibilitychange",visible);
    return()=>{cancelled=true;clearInterval(timer);document.removeEventListener("visibilitychange",visible);};
  },[key,metricId,objectId,durationSecs,pollMs]);
  const current=state.key===key?state:{view:null,status:"loading" as const,loaded:false};
  const segments=useMemo(()=>current.view?historySegments(current.view):[],[current.view]);
  return{points:segments.flat(),segments,view:current.view,status:current.status,loaded:current.loaded};
}
export {downsamplePreserveExtremes,gapThresholdSecs} from "./history-data";
