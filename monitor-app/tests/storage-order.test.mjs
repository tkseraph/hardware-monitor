import { test } from 'node:test';
import assert from 'node:assert/strict';
import { orderOverviewDisks } from '../src/storage-order.ts';
test('ZHITAI appears above Apple SSD without changing objects or input order', () => {
 const apple = { name: 'APPLE SSD', device_uid: 'synthetic-apple' };
 const zhitai = { name: 'ZHITAI TiPlus', device_uid: 'synthetic-zhitai' };
 const input = Object.freeze([apple, zhitai]);
 const output = orderOverviewDisks(input);
 assert.deepEqual(output, [zhitai, apple]);
 assert.deepEqual(input, [apple, zhitai]);
 assert.equal(output[0], zhitai);
});
test('brand matching ignores case and supports the Chinese name', () => {
 const disks = [{name:'APPLE SSD'}, {name:'zhitai SSD'}, {name:'致态 SSD'}];
 assert.deepEqual(orderOverviewDisks(disks), [disks[1], disks[2], disks[0]]);
});
test('other drives retain relative order; empty and missing-brand lists work', () => {
 const disks = [{name:'External SSD'}, {name:'APPLE SSD'}, {name:''}];
 assert.deepEqual(orderOverviewDisks(disks), disks);
 assert.deepEqual(orderOverviewDisks([]), []);
 const mixed = [disks[0], {name:'ZHITAI'}, disks[1], disks[2]];
 assert.deepEqual(orderOverviewDisks(mixed), [mixed[1], ...disks]);
});
