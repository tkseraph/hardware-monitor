import {useEffect, useState} from 'react';
import {invoke, isTauri} from '@tauri-apps/api/core';
import {LanguagePersistence, type Language} from './language';

export function useLanguage() {
  const [persistence] = useState(() => new LanguagePersistence(isTauri() ? {
    read: async () => {
      const p = await invoke<{settings:{language:string};load_error:string|null}>('get_settings');
      return {language:p.settings.language, error:p.load_error};
    },
    write: (language, migrateOnly) => invoke<string>('set_language', {language, migrateOnly}),
  } : null, {
    getItem: key => localStorage.getItem(key),
    setItem: (key,value) => localStorage.setItem(key,value),
    removeItem: key => localStorage.removeItem(key),
  }));
  const [language,setLanguage] = useState<Language>('zh');
  const [busy,setBusy] = useState(true);
  const [error,setError] = useState(false);
  const [loadAttempt,setLoadAttempt] = useState(0);
  useEffect(() => {
    let cancelled=false;
    setBusy(true);setError(false);
    persistence.load().then(value => {if(!cancelled)setLanguage(value);})
      .catch(() => {if(!cancelled)setError(true);})
      .finally(() => {if(!cancelled)setBusy(false);});
    return () => {cancelled=true;};
  },[persistence,loadAttempt]);
  useEffect(() => {document.documentElement.lang=language==='zh'?'zh-CN':'en';},[language]);
  const change = async (next:Language) => {
    if(busy)return;
    setBusy(true);setError(false);
    try {setLanguage(await persistence.save(next));}
    catch {setError(true);}
    finally {setBusy(false);}
  };
  return {language,change,busy,error,retry:()=>{if(!busy)setLoadAttempt(n=>n+1);}};
}
