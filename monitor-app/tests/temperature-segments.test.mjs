import { test } from 'node:test';
import assert from 'node:assert/strict';
import { temperatureSegments } from '../src/history-data.ts';
test('continuous flat hour stays connected despite display point gaps over 15 seconds', () => {
 const points = Array.from({length:1801}, (_,i)=>[i*2,40]);
 const segments = temperatureSegments(points);
 assert.equal(segments.length,1);
 assert.ok(segments[0].some((p,i,a)=>i && p[0]-a[i-1][0]>15));
 assert.ok(segments[0].length<=400);
 assert.deepEqual(segments[0][0],points[0]);
 assert.deepEqual(segments[0].at(-1),points.at(-1));
});
test('true missing interval remains disconnected, endpoints and spike survive', () => {
 const points = Array.from({length:1801}, (_,i)=>[i*2,i===200?60:40]).filter(([t])=>t<1000 || t>1600);
 const segments = temperatureSegments(points);
 assert.equal(segments.length,2);
 assert.equal(segments[0].at(-1)[0],998);
 assert.equal(segments[1][0][0],1602);
 assert.ok(segments[0].some(([,v])=>v===60));
});
test('empty, isolated and short histories preserve honest segments', () => {
 assert.deepEqual(temperatureSegments([]),[]);
 assert.deepEqual(temperatureSegments([[1,40],[30,40]]),[[[1,40]],[[30,40]]]);
 assert.deepEqual(temperatureSegments([[1,40],[16,40]]),[[[1,40],[16,40]]]);
});
