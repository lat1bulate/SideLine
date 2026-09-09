const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { JSDOM } = require('jsdom');
const root = path.resolve(__dirname, '..');
const source = fs.readFileSync(path.join(root, 'dist/main.js'), 'utf8');
const html = fs.readFileSync(path.join(root, 'dist/index.html'), 'utf8');
const defaults = { side: 'right', collapsed: false, ontop: true, completed_expanded: false };
const task = (text, notes = [], done = false) => ({ text, notes, done, created: 1 });
const tick = () => new Promise(resolve => setImmediate(resolve));
const deferred = () => { let resolve, reject; const promise = new Promise((a, b) => { resolve = a; reject = b; }); return { promise, resolve, reject }; };
async function app(t, fixtures = [task('甲', ['甲注']), task('乙', ['乙注'])], overrides = {}) {
  const dom = new JSDOM(html, { runScripts: 'outside-only', pretendToBeVisual: true, url: 'http://sideline.test/' });
  t.after(() => dom.window.close());
  const { window: w } = dom;
  const calls = [];
  w.__TAURI__ = { core: { invoke: (command, args) => {
    calls.push({ command, args: args && JSON.parse(JSON.stringify(args)) });
    if (overrides[command]) return Promise.resolve().then(() => overrides[command](args));
    if (command === 'load_todos') return Promise.resolve(JSON.parse(JSON.stringify(fixtures)));
    if (command === 'load_settings') return Promise.resolve({ ...defaults });
    return Promise.resolve();
  } } };
  w.eval(source);
  await tick();
  const q = selector => w.document.querySelector(selector);
  const qa = selector => [...w.document.querySelectorAll(selector)];
  const event = (el, type, init = {}) => el.dispatchEvent(new w.MouseEvent(type, { bubbles: true, cancelable: true, ...init }));
  const key = (el, value, init = {}) => el.dispatchEvent(new w.KeyboardEvent('keydown', { key: value, bubbles: true, cancelable: true, ...init }));
  const add = text => { q('#new-todo').value = text; key(q('#new-todo'), 'Enter'); };
  const saves = () => calls.filter(c => c.command === 'save_todos');
  return { w, q, qa, event, key, calls, saves, add };
}

test('A1: editing second task note is scoped to its own row', async t => {
  const a = await app(t);
  a.event(a.qa('.note-text')[1], 'dblclick');
  const editor = a.q('.note-editor');
  assert.equal(editor.closest('li').dataset.index, '1');
  editor.value = '乙改'; a.key(editor, 'Enter'); await tick();
  assert.deepEqual(a.saves().at(-1).args.todos.map(x => x.notes), [['甲注'], ['乙改']]);
});

test('A2: IME Enter does not add a task', async t => {
  const a = await app(t); const input = a.q('#new-todo');
  input.value = '中文'; a.key(input, 'Enter', { isComposing: true }); await tick();
  assert.equal(a.saves().length, 0);
  a.key(input, 'Enter'); await tick();
  assert.equal(a.saves().at(-1).args.todos.at(-1).text, '中文');
});

test('A3: note text cannot initiate task drag', async t => {
  const a = await app(t);
  a.w.document.elementFromPoint = () => a.qa('li.todo')[1];
  a.event(a.q('.note-text'), 'pointerdown', { button: 0, clientX: 0, clientY: 0 });
  a.event(a.w, 'pointermove', { clientX: 20, clientY: 20 });
  a.event(a.w, 'pointerup'); await tick();
  assert.equal(a.saves().length, 0);
  assert.deepEqual(a.qa('.text').map(el => el.textContent), ['甲', '乙']);
});

test('A4: writes are serialized immutable snapshots; exit waits for latest save', async t => {
  const first = deferred(), second = deferred(); let n = 0;
  const a = await app(t, [], { save_todos: () => (++n === 1 ? first.promise : second.promise) });
  a.add('一'); await tick(); a.add('二'); await tick();
  assert.equal(a.saves().length, 1);
  assert.deepEqual(a.saves()[0].args.todos.map(x => x.text), ['一']);
  a.q('#btn-quit').click(); await tick();
  assert.equal(a.calls.some(c => c.command === 'quit_app'), false);
  first.resolve(); await tick();
  assert.deepEqual(a.saves()[1].args.todos.map(x => x.text), ['一', '二']);
  assert.equal(a.calls.some(c => c.command === 'quit_app'), false);
  second.resolve(); await tick(); await tick();
  assert.equal(a.calls.filter(c => c.command === 'quit_app').length, 1);
});

test('A6: completed tasks occupy a counted collapsible bottom section', async t => {
  const a = await app(t, [task('完成', ['保留'], true), task('待办')]);
  assert.deepEqual(a.qa('#list .text').map(el => el.textContent), ['待办']);
  assert.equal(a.q('#completed-toggle').getAttribute('aria-expanded'), 'false');
  assert.match(a.q('#completed-toggle').textContent, /1/);
  a.q('#completed-toggle').click(); await tick();
  assert.equal(a.q('#completed-list .note-text').textContent, '保留');
  assert.equal(a.q('#completed-list li').dataset.index, '0');
});
