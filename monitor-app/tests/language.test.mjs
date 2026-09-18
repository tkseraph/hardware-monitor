import test from 'node:test';
import assert from 'node:assert/strict';
import {LanguagePersistence, LANGUAGE_KEY} from '../src/language.ts';
function cache(value) {
  const data=new Map(value ? [[LANGUAGE_KEY,value]] : []);
  return {getItem:k=>data.get(k)??null,setItem:(k,v)=>data.set(k,v),removeItem:k=>data.delete(k)};
}
test('persisted language takes precedence and removes stale browser preference',async()=>{
  const c=cache('zh');
  const p=new LanguagePersistence({read:async()=>({language:'en',error:null}),write:async()=>{throw Error('unexpected write');}},c);
  assert.equal(await p.load(),'en');assert.equal(c.getItem(LANGUAGE_KEY),null);
});
test('legacy migration clears cache only after acknowledged save and respects a concurrent saved choice',async()=>{
  const c=cache('en');let fail=true;
  const p=new LanguagePersistence({read:async()=>({language:'system',error:null}),write:async(value,migrate)=>{
    assert.equal(value,'en');assert.equal(migrate,true);if(fail)throw Error('disk blocked');return 'zh';
  }},c);
  await assert.rejects(p.load());assert.equal(c.getItem(LANGUAGE_KEY),'en');
  fail=false;assert.equal(await p.load(),'zh');assert.equal(c.getItem(LANGUAGE_KEY),null);
});
test('bad settings are surfaced without migration or overwriting browser preference',async()=>{
  const c=cache('en');
  const p=new LanguagePersistence({read:async()=>({language:'system',error:'corrupt'}),write:async()=>{throw Error('unexpected write');}},c);
  await assert.rejects(p.load(),/corrupt/);assert.equal(c.getItem(LANGUAGE_KEY),'en');
});
test('explicit choices persist through a fresh controller and failed saves keep prior state',async()=>{
  let saved='zh',fail=false;
  const backend={read:async()=>({language:saved,error:null}),write:async(value,migrate)=>{assert.equal(migrate,false);if(fail)throw Error('read only');saved=value;return value;}};
  const p=new LanguagePersistence(backend,cache());assert.equal(await p.save('en'),'en');
  assert.equal(await new LanguagePersistence(backend,cache('zh')).load(),'en');
  fail=true;await assert.rejects(p.save('zh'));assert.equal(saved,'en');
});
test('blocked browser cache does not prevent desktop reads; preview saves report failure',async()=>{
  const c={getItem:()=>{throw Error('blocked');},removeItem:()=>{throw Error('blocked');},setItem:()=>{throw Error('blocked');}};
  const p=new LanguagePersistence({read:async()=>({language:'en',error:null}),write:async()=> 'en'},c);
  assert.equal(await p.load(),'en');assert.equal(await p.save('en'),'en');
  const preview=new LanguagePersistence(null,c);assert.equal(await preview.load(),'zh');await assert.rejects(preview.save('en'));
});
