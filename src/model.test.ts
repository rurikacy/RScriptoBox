import { test } from 'node:test';
import assert from 'node:assert/strict';
import { compatibleRuntime, filterScripts, limitLog, matchesRunState, parseCodes, runEncoding, secretSlots, selectTaskRuns, type Script } from './model.ts';

const sample: Script = { id: 'one', name: '数据库备份', path: 'C:\\脚本\\备份.py', description: '每周归档', tags: ['工具'], kind: 'python', exists: true, favorite: true, args: ['', { secretSlot: 'token', required: true }], env: { TOKEN: { secretSlot: 'token', required: false } }, runtimeId: null, environmentId: null, cwd: 'C:\\脚本', runMode: 'captured', timeoutSeconds: null, encoding: 'utf-8', successCodes: [0], trustedHash: null, createdAt: '', updatedAt: '', executionPolicy: null };
test('script filters combine metadata, type, tags, favorite and presence', () => {
  assert.equal(filterScripts([sample], '每周', 'python', '工具', true, 'exists').length, 1);
  assert.equal(filterScripts([sample], '备份', 'cmd', '', false, '').length, 0);
  assert.equal(filterScripts([sample], '', '', '', false, 'missing').length, 0);
});
test('PS engines remain explicit compatible choices', () => {
  assert.equal(compatibleRuntime('powershell', 'windows_powershell'), true);
  assert.equal(compatibleRuntime('python', 'powershell'), false);
});
test('exit code validation rejects unsafe values and preserves valid codes', () => {
  assert.deepEqual(parseCodes('0, 3010，0'), [0, 3010]);
  for (const input of ['', '-1', '1.5', '4294967296', 'Infinity']) assert.throws(() => parseCodes(input));
});
test('secrets deduplicate with required constraint and preserve empty literal args', () => {
  assert.deepEqual(secretSlots(sample), [{ name: 'token', required: true }]);
  assert.equal(sample.args[0], '');
});
test('display log has bounded text and line count', () => {
  assert.equal(limitLog('x'.repeat(1_000_100)).length, 1_000_000);
  assert.equal(limitLog('line\n'.repeat(12_000)).split('\n').length, 10_000);
});
test('log replay defaults to the actual saved execution encoding', () => {
  assert.equal(runEncoding({ planRedacted: { encoding: 'system' } }), 'system');
  assert.equal(runEncoding({ planRedacted: { encoding: 'gb18030' } }), 'gb18030');
  assert.equal(runEncoding({ planRedacted: { encoding: 'unknown' } }), 'utf-8');
  assert.equal(runEncoding(undefined), 'utf-8');
});
test('cancelled history filter includes current and previous stop states', () => {
  assert.equal(matchesRunState('cancelled', 'cancelled'), true);
  assert.equal(matchesRunState('stopped', 'cancelled'), true);
  assert.equal(matchesRunState('succeeded', 'cancelled'), false);
  assert.equal(matchesRunState('failed', ''), true);
});
test('active tasks stay visible ahead of more than 100 recent completed tasks', () => {
  const runs = [...Array.from({ length: 150 }, (_, id) => ({ id, state: 'succeeded' })), { id: 151, state: 'running' }, { id: 152, state: 'waiting_children' }];
  const visible = selectTaskRuns(runs);
  assert.equal(visible.length, 100);
  assert.deepEqual(visible.slice(0, 2).map(run => run.id), [151, 152]);
  assert.equal(visible[2]!.id, 0);
});
