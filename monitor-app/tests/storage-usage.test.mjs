import { test } from 'node:test';
import assert from 'node:assert/strict';
import { storageUsage } from '../src/storage-usage.ts';
const c = (overrides = {}) => ({container_ref:'disk3', capacity_ceiling:100, capacity_free:60, capacity_in_use:null, ...overrides});
test('derives used from ceiling minus free', () => assert.deepEqual(storageUsage([c()]), {total:100,used:40,percent:40}));
test('prefers direct used and sums distinct containers', () => assert.deepEqual(storageUsage([c({capacity_in_use:20}),c({container_ref:'disk5'})]), {total:200,used:60,percent:30}));
test('preserves real zero', () => assert.equal(storageUsage([c({capacity_free:100})]).percent,0));
test('unknown and invalid capacities remain unknown', () => {
 for (const rows of [[],[c({capacity_free:null})],[c({capacity_ceiling:0})],[c({capacity_free:101})],[c({capacity_in_use:-1})],[c({capacity_in_use:NaN})],[c(),c()]]) assert.equal(storageUsage(rows),null);
});
