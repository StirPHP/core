// Tokenises templates with the extension's grammar, through the library VS Code itself uses. With
// VS Code installed, PHP and HTML are its own grammars; otherwise small stand-ins, enough for what's
// asserted here, which is what's Blade's and where PHP starts and stops.

import assert from 'node:assert/strict';
import { existsSync, readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { join } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import oniguruma from 'vscode-oniguruma';
import textmate from 'vscode-textmate';

const here = fileURLToPath(new URL('.', import.meta.url));
const require = createRequire(import.meta.url);
const SCOPE = 'text.html.blade.stirphp';

const standIns = {
  'source.php': {
    scopeName: 'source.php',
    patterns: [
      { match: '\\$\\w+', name: 'variable.other.php' },
      { begin: "'", end: "'", name: 'string.quoted.single.php' },
      { begin: '"', end: '"', name: 'string.quoted.double.php' },
    ],
  },
  'text.html.basic': {
    scopeName: 'text.html.basic',
    patterns: [
      { begin: '<!--', end: '-->', name: 'comment.block.html' },
      {
        begin: '</?[a-zA-Z][\\w:-]*',
        end: '/?>',
        name: 'meta.tag.html',
        patterns: [{ begin: '"', end: '"', name: 'string.quoted.double.html' }],
      },
    ],
  },
};

// `VSCODE_EXTENSIONS=` (empty) tests with the stand-ins
const vscode = [
  process.env.VSCODE_EXTENSIONS,
  ...(process.env.VSCODE_EXTENSIONS === '' ? [] : [
    '/usr/share/code/resources/app/extensions',
    '/usr/lib/code/extensions',
    '/opt/visual-studio-code/resources/app/extensions',
    '/Applications/Visual Studio Code.app/Contents/Resources/app/extensions',
  ]),
].find((dir) => dir && existsSync(join(dir, 'php/syntaxes/php.tmLanguage.json')));

/** The grammar of a scope; `null` for one this doesn't have, which Blade works without. */
function rawGrammar(scope) {
  const installed = {
    'source.php': 'php/syntaxes/php.tmLanguage.json',
    'text.html.basic': 'html/syntaxes/html.tmLanguage.json',
    'source.js': 'javascript/syntaxes/JavaScript.tmLanguage.json',
    'source.css': 'css/syntaxes/css.tmLanguage.json',
  };
  if (scope === SCOPE) return JSON.parse(readFileSync(join(here, '../grammars/blade.tmLanguage.json'), 'utf8'));
  if (vscode && installed[scope]) return JSON.parse(readFileSync(join(vscode, installed[scope]), 'utf8'));
  return standIns[scope] ?? null;
}

async function loadGrammar() {
  const wasm = readFileSync(require.resolve('vscode-oniguruma/release/onig.wasm'));
  await oniguruma.loadWASM(wasm.buffer.slice(wasm.byteOffset, wasm.byteOffset + wasm.byteLength));
  const registry = new textmate.Registry({
    onigLib: Promise.resolve({
      createOnigScanner: (patterns) => new oniguruma.OnigScanner(patterns),
      createOnigString: (s) => new oniguruma.OnigString(s),
    }),
    loadGrammar: async (scope) => rawGrammar(scope),
  });
  return registry.loadGrammar(SCOPE);
}

const grammar = await loadGrammar();

/** Each piece of the text with its scopes, and where it is. */
function tokenize(text) {
  const out = [];
  let stack = textmate.INITIAL;
  let offset = 0;
  for (const line of text.split('\n')) {
    const r = grammar.tokenizeLine(line, stack);
    for (const t of r.tokens) {
      out.push({ text: line.slice(t.startIndex, t.endIndex), scopes: t.scopes, end: offset + t.endIndex });
    }
    stack = r.ruleStack;
    offset += line.length + 1;
  }
  return out;
}

/** The scopes at the last character of the first `piece`: PHP's grammar has `$` apart from the name. */
function scopesOf(text, piece) {
  const at = text.indexOf(piece);
  assert.ok(at >= 0, `no ${piece} in ${text}`);
  const last = at + piece.length;
  return tokenize(text).find((t) => t.end >= last).scopes;
}

function has(text, piece, scope) {
  const scopes = scopesOf(text, piece);
  assert.ok(scopes.some((s) => s.startsWith(scope)), `${piece} in ${text}: ${scope} isn't in ${scopes.join(' ')}`);
}

function hasNot(text, piece, scope) {
  const scopes = scopesOf(text, piece);
  assert.ok(!scopes.some((s) => s.startsWith(scope)), `${piece} in ${text}: ${scope} is in ${scopes.join(' ')}`);
}

const PHP = 'source.php';

test('echoes hold PHP, and end at their delimiters', () => {
  has('<p>{{ $title }}</p>', '$title', PHP);
  has('<p>{{ $title }}</p>', '{{', 'punctuation.section.embedded.begin.blade');
  has('<p>{{ $title }}</p>', '}}', 'punctuation.section.embedded.end.blade');
  hasNot('{{ $a }} $b', '$b', PHP);
  has('{!! $html !!}', '$html', PHP);
  hasNot('{!! $html !!} $b', '$b', PHP);
  has('{{\n  $a\n}}', '$a', PHP);
  hasNot('{{\n  $a\n}} $b', '$b', PHP);
});

test('a comment is a comment, with nothing in it read', () => {
  has('{{-- {{ $a }} @if --}}', '$a', 'comment.block.blade');
  hasNot('{{-- {{ $a }} @if --}}', '$a', PHP);
  hasNot('{{-- a --}} $b', '$b', 'comment');
});

test('escapes leave what follows as text', () => {
  hasNot('@{{ $a }}', '$a', PHP);
  has('@{{ $a }}', '@', 'constant.character.escape.blade');
  hasNot('@@if ($a)', '$a', PHP);
  hasNot('@verbatim {{ $a }} @endverbatim', '$a', PHP);
  has('@verbatim\n<p>{{ $a }}</p>\n@endverbatim', '$a', 'meta.verbatim.blade');
  has('@verbatim x @endverbatim {{ $b }}', '$b', PHP);
});

test('directives', () => {
  has('@foreach ($posts as $post)', '@foreach', 'keyword.control.directive.blade');
  has('@foreach ($posts as $post)', '$posts', PHP);
  has('@endforeach', '@endforeach', 'keyword.control.directive.blade');
  has('@ENDIF', '@ENDIF', 'keyword.control.directive.blade');
  has('@include("a", ["b" => $c])', '@include', 'keyword.other.directive.blade');
  has('@include("a", ["b" => $c])', '$c', PHP);
  has('@csrf', '@csrf', 'keyword.other.directive.blade');
  has('@if (count($a) > 0) $b', '$a', PHP);
  hasNot('@if (count($a) > 0) $b', '$b', PHP);
  has('@if ($a)\n  x\n@endif', '@endif', 'keyword.control.directive.blade');
});

test('arguments that span lines, and a directive that takes none', () => {
  has('@include(\n  "a",\n  [$b]\n) $c', '$b', PHP);
  hasNot('@include(\n  "a",\n  [$b]\n) $c', '$c', PHP);
  hasNot('@else ($a)', '$a', PHP);
});

test("what isn't a directive stays HTML", () => {
  for (const text of ['@media (min-width: 1px)', '@click', 'foo@if.com', '@iffy', '@custom($a)']) {
    assert.ok(
      tokenize(text).every((t) => !t.scopes.some((s) => s.includes('directive') || s === PHP)),
      `${text}: ${JSON.stringify(tokenize(text))}`,
    );
  }
});

test('@use is a name, not PHP', () => {
  has("@use('App\\Models\\Post')", 'App', 'string.unquoted.use.blade');
  hasNot("@use('App\\Models\\Post')", 'App', PHP);
  has("@use('App\\Post') {{ $a }}", '$a', PHP);
});

test('@php and <?php', () => {
  has('@php\n  $a = 1;\n@endphp', '$a', PHP);
  has('@php\n  $a = 1;\n@endphp', '@endphp', 'keyword.other.directive.blade');
  hasNot('@php\n  $a = 1;\n@endphp\n$b', '$b', PHP);
  has('@php($a = 1)', '$a', PHP);
  hasNot('@php($a = 1) $b', '$b', PHP);
  has('<?php echo $a; ?>', '$a', PHP);
  hasNot('<?php echo $a; ?> $b', '$b', PHP);
  has('<?= $a ?>', '$a', PHP);
});

test('inside HTML: attributes, comments and scripts', () => {
  has('<a href="{{ $u }}">', '$u', PHP);
  hasNot('<a href="{{ $u }}" title="$v">', '$v', PHP);
  has('<a title="@lang($a)">', '$a', PHP);
  has('<!-- {{ $a }} -->', '$a', PHP);
  has('<div @class(["a" => $b])>', '$b', PHP);
  has('<div @style(["a" => $b]) id="x">', '$b', PHP);
  hasNot('<div :class="$a">', '$a', PHP);
  hasNot('<div @click="$a">', '$a', PHP);
  has('<script>\nlet a = {{ $b }};\n</script>', '$b', PHP);
});

test('component tags', () => {
  has('<x-alert />', 'x-alert', 'entity.name.tag.component.blade');
  has('<x-forms.input />', 'x-forms.input', 'entity.name.tag.component.blade');
  has('<x-pkg::alert />', 'x-pkg::alert', 'entity.name.tag.component.blade');
  has('<x-slot:title>', 'x-slot:title', 'entity.name.tag.component.blade');
  has('</x-alert>', 'x-alert', 'entity.name.tag.component.blade');
  has('<x:alert/>', 'x:alert', 'entity.name.tag.component.blade');
  hasNot('<x-alert /> $b', '$b', 'meta.tag.component.blade');
  hasNot('<xylophone>', 'xylophone', 'entity.name.tag.component.blade');
});

test("a component's attributes", () => {
  const bound = '<x-alert :count="$a" ::text="$b" type="$c" name="{{ $d }}" :$e {{ $attributes }} @class([$f]) />  $g';
  has(bound, '$a', PHP);
  has(bound, ':count', 'entity.other.attribute-name.blade');
  hasNot(bound, '$b', PHP);
  has(bound, '::text', 'entity.other.attribute-name.html');
  hasNot(bound, '$c', PHP);
  has(bound, '$c', 'string.quoted.double.html');
  has(bound, '$d', PHP);
  has(bound, '$e', 'variable.other.php');
  has(bound, '$attributes', PHP);
  has(bound, '$f', PHP);
  hasNot(bound, '$g', PHP);
  has("<x-alert\n  :items=\"['a' => $x]\"\n  title='{{ $y }}'\n/>", '$x', PHP);
  has("<x-alert\n  :items=\"['a' => $x]\"\n  title='{{ $y }}'\n/>", '$y', PHP);
});

test('PHP that nests keeps its end', () => {
  has("{{ route('a', ['b' => $c]) }} $d", '$c', PHP);
  hasNot("{{ route('a', ['b' => $c]) }} $d", '$d', PHP);
  hasNot('{{ new Foo($a) }} $d', '$d', PHP);
  hasNot('{{ $a ? fn($b) => $b : null }} $d', '$d', PHP);
  hasNot("@if (str_contains($a, ')')) $d", '$d', PHP);
  hasNot('@foreach (array_map(fn($x) => ($x), $y) as $z) $d', '$d', PHP);
  hasNot('{{ match($a) { 1 => 2 } }} $d', '$d', PHP);
  has('@php\n  function f(): void { echo $a; }\n@endphp\n@endif', '@endif', 'keyword.control.directive.blade');
});

test('the language configuration is valid, with regular expressions that compile', () => {
  const config = JSON.parse(readFileSync(join(here, '../grammars/blade.language-configuration.json'), 'utf8'));
  const { increaseIndentPattern: more, decreaseIndentPattern: less } = config.indentationRules;
  assert.match('@foreach ($a as $b)', new RegExp(more.pattern, more.flags));
  assert.doesNotMatch('@if ($a) x @endif', new RegExp(more.pattern, more.flags));
  assert.match('@ENDFOREACH', new RegExp(less.pattern, less.flags));
  assert.match('$post', new RegExp(config.wordPattern));
});
