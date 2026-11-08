// What bundlers need of `index.js` to find the worker and the `.wasm` it loads: `node --test`.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';

test('the worker is started with options a bundler can read', () => {
  const source = readFileSync(new URL('../index.js', import.meta.url), 'utf8');
  // Vite evaluates the options statically, and fails the build on anything but literals
  const call = source.match(/new Worker\(new URL\('\.\/worker\.js', import\.meta\.url\), (\{[^}]*\})\)/);
  assert.ok(call, 'startWorker() creates the worker from a literal URL');
  assert.deepEqual(new Function(`return ${call[1]}`)(), { type: 'module', name: 'stirphp' });
  assert.doesNotMatch(call[1], /options|\?\?/);
});
