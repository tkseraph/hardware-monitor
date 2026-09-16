import {test} from 'node:test';
import assert from 'node:assert/strict';
import {historySegments} from '../src/history-view.ts';
const point=(t,value=1)=>({t,value,min:value,max:value,count:1,granularity_secs:1,first_ts:t,last_ts:t});
test('server segment boundaries survive large display spacing',()=>{
  assert.deepEqual(historySegments({version:2,segments:[[point(0),point(600)],[point(1200)]]}),[[[0,1],[600,1]],[[1200,1]]]);
});
test('invalid protocol, values and unbounded responses are rejected',()=>{
  for(const view of [{version:1,segments:[]},{version:2,segments:[[point(0,NaN)]]},{version:2,segments:[[point(2),point(1)]]},{version:2,segments:[Array.from({length:2001},(_,i)=>point(i))]}])assert.throws(()=>historySegments(view));
});
