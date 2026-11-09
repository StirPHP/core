// The real `.wasm`, in a worker thread, spoken to as an editor would: `node --test`.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { Worker } from 'node:worker_threads';

const root = 'file:///project';
const files = {
  '/project/composer.json': JSON.stringify({
    require: { php: '^8.2' },
    autoload: { 'psr-4': { 'App\\': 'src/' } },
  }),
  '/project/stirphp.neon':
    'parameters:\n    stirphp:\n        rules:\n            - rules/no_var_dump.rhai\n',
  '/project/rules/no_var_dump.rhai':
    'fn visit_func_call(node, ctx) {\n    if node.name == "var_dump" { ctx.error("no var_dump", "app.noVarDump"); }\n}\n',
  '/project/src/Greeter.php':
    '<?php\nnamespace App\\Greeting;\n\nfinal class Greeter\n{\n    public function greet(string $name): string\n    {\n        return "Hello, $name";\n    }\n}\n',
};
const main = `${root}/src/main.php`;
const text = '<?php\nnamespace App;\n\n$g = new Greeter();\necho $g->greet(42);\nvar_dump($g);\n';

/** A client of the worker: requests by id, and every notification kept. */
function client() {
  const worker = new Worker(new URL('./node-worker.mjs', import.meta.url));
  const waiting = new Map();
  const notes = [];
  const watchers = [];
  let id = 0;
  worker.on('message', (m) => {
    if (process.env.SHOW) console.log(JSON.stringify(m).slice(0, 300));
    if (m.id !== undefined && m.method === undefined) {
      waiting.get(m.id)?.(m);
      waiting.delete(m.id);
      return;
    }
    notes.push(m);
    for (const w of [...watchers]) w();
  });
  worker.on('error', (e) => {
    throw e;
  });
  return {
    worker,
    notes,
    request(method, params) {
      id += 1;
      const mine = id;
      return new Promise((resolve) => {
        waiting.set(mine, resolve);
        worker.postMessage({ jsonrpc: '2.0', id: mine, method, params });
      });
    },
    notify(method, params) {
      worker.postMessage({ jsonrpc: '2.0', method, params });
    },
    /** The next notification that `pick` takes, from those that came in already first. */
    until(pick) {
      return new Promise((resolve) => {
        const look = () => {
          const i = notes.findIndex(pick);
          if (i < 0) return;
          watchers.splice(watchers.indexOf(look), 1);
          resolve(notes.splice(i, 1)[0]);
        };
        watchers.push(look);
        look();
      });
    },
  };
}

const diagnosticsOf = (uri) => (m) =>
  m.method === 'textDocument/publishDiagnostics' && m.params.uri === uri;

test('diagnostics, completion with an import, and rename across files', async () => {
  const c = client();
  const started = performance.now();
  const init = await c.request('initialize', {
    processId: null,
    rootUri: root,
    capabilities: {},
    initializationOptions: { files },
  });
  const ready = performance.now() - started;
  assert.equal(init.result.serverInfo.name, 'stirphp');
  c.notify('initialized', {});
  c.notify('textDocument/didOpen', {
    textDocument: { uri: main, languageId: 'php', version: 1, text },
  });

  const opened = performance.now();
  const first = await c.until(diagnosticsOf(main));
  const firstDiagnostics = performance.now() - opened;
  const codes = first.params.diagnostics.map((d) => d.code).sort();
  assert.deepEqual(codes, ['app.noVarDump', 'class.notFound'], JSON.stringify(first));

  // the class, with its `use` line
  c.notify('textDocument/didChange', {
    textDocument: { uri: main, version: 2 },
    contentChanges: [{ text: '<?php\nnamespace App;\n\n$g = new Gree\n' }],
  });
  const asked = performance.now();
  const completion = await c.request('textDocument/completion', {
    textDocument: { uri: main },
    position: { line: 3, character: 13 },
  });
  const completionTime = performance.now() - asked;
  const item = completion.result.items.find((i) => i.label === 'Greeter');
  assert.ok(item, JSON.stringify(completion.result.items.map((i) => i.label)));
  assert.match(item.additionalTextEdits[0].newText, /^\s*use App\\Greeting\\Greeter;\s*$/);

  // an edit fixes it: diagnostics follow
  c.notify('textDocument/didChange', {
    textDocument: { uri: main, version: 3 },
    contentChanges: [{ text: '<?php\nnamespace App;\n\nuse App\\Greeting\\Greeter;\n\n$g = new Greeter();\necho $g->greet("you");\n' }],
  });
  const typed = performance.now();
  const fixed = await c.until((m) => diagnosticsOf(main)(m) && m.params.diagnostics.length === 0);
  const editToDiagnostics = performance.now() - typed;
  assert.equal(fixed.params.diagnostics.length, 0);

  const rename = await c.request('textDocument/rename', {
    textDocument: { uri: main },
    position: { line: 6, character: 10 },
    newName: 'welcome',
  });
  const changed = Object.keys(rename.result.changes).sort();
  assert.deepEqual(changed, [`${root}/src/Greeter.php`, main]);

  // and the text of a builtin, for going to its definition
  const stub = await c.request('stirphp/fileText', { uri: 'stirphp:///stubs/Core/Core.php' });
  assert.match(stub.result, /function strlen/);

  console.log(
    `initialize ${ready.toFixed(0)} ms, first diagnostics ${firstDiagnostics.toFixed(0)} ms, ` +
      `completion ${completionTime.toFixed(0)} ms, edit to diagnostics ${editToDiagnostics.toFixed(0)} ms`,
  );
  await c.request('shutdown', null);
  c.notify('exit', null);
  await c.worker.terminate();
});

test('a template, by its name and whatever its language is called', async () => {
  const c = client();
  await c.request('initialize', {
    processId: null,
    rootUri: root,
    capabilities: {},
    initializationOptions: { files: { '/project/composer.json': '{}' } },
  });
  c.notify('initialized', {});
  const uri = `${root}/page.blade.php`;
  const page = '<p @click="a">{{ strlen(1, 2, 3) }}</p>\n@fore\n';
  c.notify('textDocument/didOpen', {
    textDocument: { uri, languageId: 'blade', version: 1, text: page },
  });
  const diagnostics = await c.until(diagnosticsOf(uri));
  const lines = diagnostics.params.diagnostics.map((d) => d.range.start.line);
  assert.ok(lines.includes(0), JSON.stringify(diagnostics));

  const hover = await c.request('textDocument/hover', {
    textDocument: { uri },
    position: { line: 0, character: 19 },
  });
  assert.match(JSON.stringify(hover.result), /strlen/);

  const completion = await c.request('textDocument/completion', {
    textDocument: { uri },
    position: { line: 1, character: 5 },
    context: { triggerKind: 1 },
  });
  assert.ok(completion.result.items.some((i) => i.label === '@foreach'), JSON.stringify(completion.result));
  await c.request('shutdown', null);
  c.notify('exit', null);
  await c.worker.terminate();
});
