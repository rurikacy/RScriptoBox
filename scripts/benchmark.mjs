import { performance } from 'node:perf_hooks';
import assert from 'node:assert/strict';
import { filterScripts } from '../src/model.ts';

const scripts = Array.from({ length: 1000 }, (_, index) => ({
  id: String(index), name: '脚本 ' + index, path: 'D:\\Scripts\\tool-' + index + '.py',
  kind: 'python', description: '用于测试本地脚本库搜索性能',
  tags: ['效率工具', index % 2 ? '开发' : '文件整理'], favorite: index % 4 === 0, exists: true,
}));
const samples = [];
for (let index = 0; index < 120; index++) {
  const begin = performance.now();
  const result = filterScripts(scripts, '测试', 'python', '开发', false, '');
  assert.equal(result.length, 500);
  if (index >= 20) samples.push(performance.now() - begin);
}
samples.sort((a, b) => a - b);
const p95 = samples[Math.ceil(samples.length * .95) - 1];
console.log(JSON.stringify({ records: scripts.length, iterations: samples.length,
  searchP95Ms: Number(p95.toFixed(3)), scope: 'filter logic only; excludes IPC and rendering' }, null, 2));
assert.ok(p95 < 200, 'Search exceeds the design target');
