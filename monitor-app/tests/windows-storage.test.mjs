import {test} from 'node:test';
import assert from 'node:assert/strict';
import {groupVolumes,volumeUsage,rateForDisk} from '../src/windows-storage.ts';
const disks=[{number:0,device_uid:'synthetic-a'},{number:1,device_uid:'synthetic-b'}];
const volume=(disk_numbers,mapping_state='ok')=>({alias:'synthetic',disk_numbers,mapping_state,size_bytes:100,free_bytes:40});
test('cross-disk and unknown volumes are counted only in their own section',()=>{
  const a=volume([0]),b=volume([0,1]),c=volume([9]),d=volume([1],'error');
  const grouped=groupVolumes(disks,[a,b,c,d]);
  assert.deepEqual(grouped.dedicated[0],[a]);assert.equal(grouped.dedicated[1],undefined);assert.deepEqual(grouped.shared,[b,c,d]);
});
test('capacity keeps true zero and rejects mismatched or unavailable values',()=>{
  assert.deepEqual(volumeUsage(volume([0])),{used:60,percent:60});
  assert.equal(volumeUsage({...volume([0]),free_bytes:100}).percent,0);
  for(const free of [null,-1,101,NaN])assert.equal(volumeUsage({...volume([0]),free_bytes:free}),null);
});
test('a reused disk number cannot display another connection performance',()=>{
  const old={number:0,device_uid:'old',read_bps:123};
  assert.equal(rateForDisk(disks[0],{disks:[old]}),undefined);
  assert.equal(rateForDisk({...disks[0],device_uid:null},{disks:[{...old,device_uid:null}]}),undefined);
  const current={...old,device_uid:'synthetic-a'};assert.equal(rateForDisk(disks[0],{disks:[current]}),current);
});
