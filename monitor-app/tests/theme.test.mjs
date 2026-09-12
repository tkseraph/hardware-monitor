import { test } from 'node:test';
import assert from 'node:assert/strict';
import { parseTheme, resolveTheme } from '../src/theme.ts';
test('missing and invalid saved preferences fall back to system',()=>{
 for(const value of [null,'','invalid','system']) assert.equal(parseTheme(value),'system');
 assert.equal(parseTheme('dark'),'dark');assert.equal(parseTheme('light'),'light');
});
test('explicit light and dark override either OS appearance',()=>{
 for(const osDark of [true,false]) {
  assert.equal(resolveTheme('light',osDark),'light');
  assert.equal(resolveTheme('dark',osDark),'dark');
 }
});
test('system preference follows OS appearance',()=>{
 assert.equal(resolveTheme('system',true),'dark');assert.equal(resolveTheme('system',false),'light');
});
