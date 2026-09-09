'use strict';
const invoke = window.__TAURI__.core.invoke;
const $ = id => document.getElementById(id);
const listEl = $('list');
const completedList = $('completed-list');
const inputEl = $('new-todo');
const ctxMenu = $('ctx-menu');
let todos = [];
let settings = { side: 'right', collapsed: false, ontop: true, completed_expanded: false };
let exiting = false;
let exitError = '';
let recoveryNotice = '';
let deletion = null;
let activeEditor = null;
let drag = null;
const taskViews = new Map();
const noteRecords = new WeakMap();
const composing = new WeakSet();
const message = error => error instanceof Error ? error.message : String(error);
const snapshot = value => JSON.parse(JSON.stringify(value));

// One independent, serialized writer per domain. Coalesce only not-yet-started
// snapshots; an IPC argument is never a live mutable array. Failure retains the
// newest snapshot and pauses this writer until the user explicitly retries.
class SaveQueue {
  constructor(command, argument, label, statusId) {
    Object.assign(this, { command, argument, label, statusId, ready: false,
      loading: true, loadError: '', error: '', revision: 0, saved: 0,
      latest: null, running: null });
  }
  enqueue(value) {
    if (!this.ready) return;
    this.latest = snapshot(value);
    this.revision++;
    this.pump();
    updateStatus();
  }
  pump() {
    if (this.running || this.error || !this.ready || this.saved === this.revision) return this.running;
    this.running = (async () => {
      while (!this.error && this.saved < this.revision) {
        const revision = this.revision;
        const value = this.latest;
        try {
          await invoke(this.command, { [this.argument]: value });
          this.saved = revision;
        } catch (error) {
          this.error = message(error);
        }
      }
    })().finally(() => {
      this.running = null;
      // An edit may land in a microtask after the loop finished but before this
      // finally handler. Restart for that queued revision instead of stalling.
      this.pump();
      updateStatus();
    });
    return this.running;
  }
  retry() {
    this.error = '';
    this.pump();
    updateStatus();
  }
  async flush() {
    // A failed initial read has no edits to lose; allow an ordinary exit while
    // keeping unreadable files untouched. Dirty failed saves still block exit.
    if (!this.loading && !this.ready && this.revision === 0 && !this.running) return;
    if (this.loading || !this.ready || this.loadError) throw new Error(`${this.label}尚未成功读取`);
    while (this.saved < this.revision || this.running) {
      if (this.error) throw new Error(`${this.label}保存失败：${this.error}`);
      await (this.running || this.pump());
    }
    if (this.error) throw new Error(`${this.label}保存失败：${this.error}`);
  }
}
const todoQueue = new SaveQueue('save_todos', 'todos', '待办', 'todos-status');
const settingsQueue = new SaveQueue('save_settings', 'settings', '设置', 'settings-status');
const queues = [todoQueue, settingsQueue];
const persist = () => todoQueue.enqueue(todos);
const canEdit = () => todoQueue.ready && !exiting;
let errorDisplay = false;
let errorDisplayWork = Promise.resolve();
function requestErrorDisplay(visible) {
  if (errorDisplay === visible) return;
  errorDisplay = visible;
  // CSS cannot expand a native 26px strip. This transient native override does
  // not write settings and restores the saved dock state when errors clear.
  errorDisplayWork = errorDisplayWork.catch(() => {}).then(() =>
    invoke('set_error_display', { visible })
  ).catch(error => console.error('错误提示窗口调整失败：', error));
}

function updateStatus() {
  queues.forEach(queue => {
    const el = $(queue.statusId);
    el.textContent = queue.loading ? `${queue.label}读取中…`
      : queue.loadError ? `${queue.label}读取失败`
      : queue.error ? `${queue.label}保存失败：${queue.error}`
      : queue.saved < queue.revision ? `${queue.label}保存中…` : `${queue.label}已保存`;
    el.classList.toggle('error', !!(queue.error || queue.loadError));
    $(`${queue.argument}-retry-save`).hidden = !queue.error;
    $(`${queue.argument}-retry-save`).disabled = exiting;
    $(`${queue.argument}-preserve-reload`).hidden = !queue.error;
    $(`${queue.argument}-preserve-reload`).disabled = exiting;
  });
  $('todos-load-error').hidden = !todoQueue.loadError;
  $('todos-load-message').textContent = todoQueue.loadError;
  $('settings-load-error').hidden = !settingsQueue.loadError;
  $('settings-load-message').textContent = settingsQueue.loadError;
  $('todos-retry-load').disabled = todoQueue.loading || exiting;
  $('todos-recover').disabled = todoQueue.loading || exiting;
  $('settings-retry-load').disabled = settingsQueue.loading || exiting;
  inputEl.disabled = $('btn-add').disabled = !canEdit();
  $('btn-collapse').disabled = !settingsQueue.ready || exiting;
  $('completed-toggle').disabled = exiting;
  $('btn-quit').disabled = exiting;
  $('undo-delete').hidden = !deletion;
  $('undo-delete').disabled = !canEdit();
  $('undo-message').textContent = deletion ? (deletion.kind === 'task' ? '已删除待办' : '已删除注释') : '';
  $('exit-error').textContent = exitError;
  $('exit-error').hidden = !exitError;
  $('recovery-notice').textContent = recoveryNotice;
  $('recovery-notice').hidden = !recoveryNotice;
  document.body.classList.toggle('exiting', exiting);
  // A failed save must remain visible even if the requested state was collapsed.
  // This is a temporary UI override, not a write of default settings.
  const hasError = !!(queues.some(q => q.error || q.loadError) || exitError);
  requestErrorDisplay(hasError);
  document.body.classList.toggle('collapsed', settingsQueue.ready && settings.collapsed && !hasError);
}

function validateTodos(value) {
  if (!Array.isArray(value) || value.some(t => !t || typeof t.text !== 'string'
    || typeof t.done !== 'boolean' || typeof t.created !== 'number'
    || (t.notes !== undefined && (!Array.isArray(t.notes) || t.notes.some(n => typeof n !== 'string'))))) {
    throw new Error('待办格式无效；未覆盖原文件');
  }
  return value;
}
function validateSettings(value) {
  if (!value || !['left', 'right'].includes(value.side)
    || ['collapsed', 'ontop', 'completed_expanded'].some(k => typeof value[k] !== 'boolean')) {
    throw new Error('设置格式无效；未覆盖原文件');
  }
  return value;
}
async function loadTodos(recover = false) {
  if (todoQueue.ready || (todoQueue.loading && todoQueue.started)) return;
  todoQueue.started = true;
  todoQueue.loading = true;
  updateStatus();
  try {
    todos = validateTodos(await invoke(recover ? 'recover_todos' : 'load_todos'));
    todoQueue.ready = true;
    todoQueue.loadError = '';
    render();
  } catch (error) { todoQueue.loadError = message(error); }
  finally { todoQueue.loading = false; updateStatus(); }
  if (todoQueue.ready && !document.body.classList.contains('collapsed')) inputEl.focus();
}
async function loadSettings() {
  if (settingsQueue.ready || (settingsQueue.loading && settingsQueue.started)) return;
  settingsQueue.started = true;
  settingsQueue.loading = true;
  updateStatus();
  try {
    settings = validateSettings(await invoke('load_settings'));
    settingsQueue.ready = true;
    settingsQueue.loadError = '';
    applySettings();
  } catch (error) { settingsQueue.loadError = message(error); }
  finally { settingsQueue.loading = false; updateStatus(); }
}
function applySettings() {
  const expanded = settings.completed_expanded;
  completedList.hidden = !expanded;
  $('completed-toggle').setAttribute('aria-expanded', String(expanded));
  $('completed-arrow').textContent = expanded ? '▾' : '▸';
  ctxMenu.querySelector('[data-action="ontop"]').textContent = settings.ontop ? '置顶显示 ✓' : '置顶显示';
  ['left', 'right'].forEach(side => {
    ctxMenu.querySelector(`[data-action="${side}"]`).textContent = `停靠${side === 'left' ? '左' : '右'}侧${settings.side === side ? ' ✓' : ''}`;
  });
  $('collapsed-hint').textContent = settings.side === 'left' ? '»' : '«';
  updateStatus();
}
function changeSettings(change) {
  if (!settingsQueue.ready || exiting || !finishEditor()) return;
  settings = { ...settings, ...change };
  settingsQueue.enqueue(settings);
  applySettings();
}

// Stable task and note identities (including duplicate note strings). Numeric
// dataset indexes are display/drag metadata, never closure-captured edit keys.
function notesOf(todo) {
  if (!noteRecords.has(todo)) noteRecords.set(todo, (todo.notes || []).map(text => ({ text })));
  return noteRecords.get(todo);
}
function syncNotes(todo) { todo.notes = notesOf(todo).map(note => note.text); }
function reconcile(parent, nodes) {
  const keep = new Set(nodes);
  [...parent.children].forEach(child => { if (!keep.has(child)) child.remove(); });
  let cursor = parent.firstElementChild;
  nodes.forEach(node => {
    if (node === cursor) cursor = cursor.nextElementSibling;
    else parent.insertBefore(node, cursor);
  });
}
function button(className, text, title, action) {
  const el = document.createElement('button');
  el.type = 'button'; el.className = className; el.textContent = text;
  el.title = title; el.setAttribute('aria-label', title);
  el.addEventListener('click', action);
  return el;
}
function trackComposition(input) {
  input.addEventListener('compositionstart', () => composing.add(input));
  input.addEventListener('compositionend', () => {
    composing.delete(input);
    // Blur during IME waits until its final value exists, never commits mid-IME.
    if (activeEditor?.input === input && activeEditor.blurred) {
      queueMicrotask(() => {
        if (activeEditor?.input === input && document.activeElement !== input) activeEditor.finish();
      });
    }
  });
}
function isComposing(event, input) {
  return composing.has(input) || event.isComposing || event.keyCode === 229;
}
function finishEditor(cancel = false) {
  if (!activeEditor) return true;
  return activeEditor.finish(cancel);
}
function openEditor(todo, anchor, value, className, commit, temporary = null) {
  if (!canEdit() || !finishEditor()) { temporary?.remove(); return; }
  const input = document.createElement('input');
  input.type = 'text'; input.className = className; input.value = value;
  input.placeholder = '输入注释，回车保存';
  input.setAttribute('aria-label', className === 'inline-editor' ? '编辑待办' : '编辑注释');
  anchor.replaceWith(input);
  let finished = false;
  const editor = { input, todo, blurred: false, finish(cancel = false) {
    if (finished) return true;
    if (composing.has(input) && !cancel) return false;
    finished = true;
    activeEditor = null;
    if (temporary) temporary.remove(); else input.replaceWith(anchor);
    if (!cancel && todos.includes(todo)) commit(input.value.trim());
    render();
    return true;
  } };
  activeEditor = editor;
  trackComposition(input);
  input.addEventListener('keydown', event => {
    if (isComposing(event, input)) return;
    if (event.key === 'Enter' || event.key === 'Escape') {
      event.preventDefault(); event.stopPropagation();
      editor.finish(event.key === 'Escape');
    }
  });
  input.addEventListener('blur', () => { editor.blurred = true; editor.finish(); });
  input.addEventListener('focus', () => { editor.blurred = false; });
  input.focus(); input.select();
}
function beginTextEdit(todo) {
  if (!canEdit() || !finishEditor()) return;
  const view = taskViews.get(todo);
  if (view) openEditor(todo, view.text, todo.text, 'inline-editor', value => {
    if (value && value !== todo.text) { todo.text = value; persist(); }
  });
}
function beginNoteEdit(todo, note) {
  if (!canEdit() || !finishEditor()) return;
  const view = taskViews.get(todo)?.notes.get(note);
  if (!view || !notesOf(todo).includes(note)) return;
  openEditor(todo, view.text, note.text, 'note-editor', value => {
    if (!value) removeNote(todo, note, false);
    else if (value !== note.text) { note.text = value; syncNotes(todo); persist(); }
  });
}
function beginAddNote(todo) {
  if (!canEdit() || !finishEditor()) return;
  const row = document.createElement('div'); row.className = 'note-editor-row';
  const anchor = document.createElement('span'); row.append(anchor);
  taskViews.get(todo).li.append(row);
  openEditor(todo, anchor, '', 'note-editor', value => {
    if (value) { notesOf(todo).push({ text: value }); syncNotes(todo); persist(); }
  }, row);
}
function removeNote(todo, note, finish = true) {
  if (!canEdit() || (finish && !finishEditor())) return;
  const notes = notesOf(todo); const index = notes.indexOf(note);
  if (index < 0 || !todos.includes(todo)) return;
  notes.splice(index, 1);
  deletion = { kind: 'note', todo, note, index };
  syncNotes(todo); persist(); render();
}
function removeTask(todo) {
  if (!canEdit() || !finishEditor()) return;
  const index = todos.indexOf(todo);
  if (index < 0) return;
  todos.splice(index, 1);
  deletion = { kind: 'task', todo, index };
  persist(); render();
}
function undoDeletion() {
  if (!canEdit() || !deletion || !finishEditor()) return;
  const item = deletion; deletion = null;
  if (item.kind === 'task') todos.splice(Math.min(item.index, todos.length), 0, item.todo);
  else if (todos.includes(item.todo)) {
    notesOf(item.todo).splice(Math.min(item.index, notesOf(item.todo).length), 0, item.note);
    syncNotes(item.todo);
  }
  persist(); render();
}
function createTaskView(todo) {
  const li = document.createElement('li'); li.className = 'todo';
  const row = document.createElement('div'); row.className = 'todo-row';
  const handle = document.createElement('span');
  handle.className = 'handle'; handle.textContent = '⠿'; handle.title = '拖动调整优先级';
  const checkbox = document.createElement('input'); checkbox.type = 'checkbox'; checkbox.className = 'check';
  checkbox.setAttribute('aria-label', '完成待办');
  checkbox.addEventListener('change', () => {
    const checked = checkbox.checked;
    if (!canEdit() || !finishEditor()) { checkbox.checked = todo.done; return; }
    todo.done = checked; persist(); render();
  });
  const text = document.createElement('span'); text.className = 'text'; text.title = '双击或右键编辑';
  text.addEventListener('dblclick', () => beginTextEdit(todo));
  row.append(handle, checkbox, text,
    button('row-btn btn-note', '＋', '加注释', () => beginAddNote(todo)),
    button('row-btn del', '×', '删除待办', () => removeTask(todo)));
  const notesBox = document.createElement('div'); notesBox.className = 'notes';
  li.append(row, notesBox);
  li.addEventListener('pointerdown', event => {
    if (event.button !== 0 || drag || todo.done || !canEdit()) return;
    if (!event.target.closest('.todo-row') || event.target.closest('button, input, .notes, .note-line, .note-text, .note-editor-row')) return;
    if (!finishEditor()) return;
    drag = { todo, moved: false, changed: false, startX: event.clientX, startY: event.clientY };
  });
  return { li, text, checkbox, notesBox, notes: new Map() };
}
function render() {
  // Remove deleted nodes before reconciliation, so deleting a sibling does not
  // detach/reinsert the focused editor. Unchanged rows/notes are never rebuilt.
  for (const [todo, view] of taskViews) {
    if (!todos.includes(todo)) { view.li.remove(); taskViews.delete(todo); }
  }
  const pending = [], completed = [];
  todos.forEach((todo, index) => {
    let view = taskViews.get(todo);
    if (!view) { view = createTaskView(todo); taskViews.set(todo, view); }
    view.li.dataset.index = String(index);
    view.li.classList.toggle('done', todo.done);
    view.checkbox.checked = todo.done;
    view.text.textContent = todo.text;
    const notes = notesOf(todo);
    for (const [note, line] of view.notes) {
      if (!notes.includes(note)) { line.el.remove(); view.notes.delete(note); }
    }
    const lines = notes.map((note, ni) => {
      let line = view.notes.get(note);
      if (!line) {
        const el = document.createElement('div'); el.className = 'note-line';
        const text = document.createElement('span'); text.className = 'note-text';
        text.addEventListener('dblclick', () => beginNoteEdit(todo, note));
        el.append(text, button('note-del', '×', '删除注释', () => removeNote(todo, note)));
        line = { el, text }; view.notes.set(note, line);
      }
      line.el.dataset.ni = String(ni); line.text.textContent = note.text;
      return line.el;
    });
    reconcile(view.notesBox, lines);
    view.notesBox.hidden = !notes.length;
    (todo.done ? completed : pending).push(view.li);
  });
  reconcile(listEl, pending); reconcile(completedList, completed);
  $('count').textContent = `${pending.length} 项待办 / 共 ${todos.length} 项`;
  $('completed-count').textContent = `已完成 ${completed.length}`;
  $('empty-list').hidden = !todoQueue.ready || pending.length > 0;
  updateStatus();
}

function endDrag() {
  if (!drag) return;
  const changed = drag.changed;
  drag = null;
  document.querySelectorAll('.dragging').forEach(el => el.classList.remove('dragging'));
  if (changed) persist();
}
window.addEventListener('pointermove', event => {
  if (!drag || exiting) return;
  if (!drag.moved && Math.hypot(event.clientX - drag.startX, event.clientY - drag.startY) < 5) return;
  drag.moved = true; event.preventDefault();
  const li = document.elementFromPoint(event.clientX, event.clientY)?.closest('#list li.todo');
  if (!li) return;
  const target = todos[Number(li.dataset.index)];
  const from = todos.indexOf(drag.todo), to = todos.indexOf(target);
  if (from < 0 || to < 0 || target.done || from === to) return;
  todos.splice(from, 1); todos.splice(to, 0, drag.todo);
  drag.changed = true; render();
  taskViews.get(drag.todo).li.classList.add('dragging');
});
['pointerup', 'pointercancel', 'blur'].forEach(type => window.addEventListener(type, endDrag));

function addTodo() {
  if (!canEdit() || composing.has(inputEl) || !finishEditor()) return;
  const text = inputEl.value.trim();
  if (!text) return;
  todos.push({ text, done: false, created: Date.now(), notes: [] });
  inputEl.value = ''; persist(); render(); inputEl.focus();
}
trackComposition(inputEl);
$('btn-add').addEventListener('click', addTodo);
inputEl.addEventListener('keydown', event => {
  if (isComposing(event, inputEl)) return;
  if (event.key === 'Enter') { event.preventDefault(); addTodo(); }
  else if (event.key === 'Escape') { event.preventDefault(); inputEl.value = ''; }
});
$('undo-delete').addEventListener('click', undoDeletion);
document.addEventListener('keydown', event => {
  if ((event.ctrlKey || event.metaKey) && !event.shiftKey && !event.altKey && event.key.toLowerCase() === 'z'
    && !event.isComposing && event.keyCode !== 229
    && !event.target.closest('input, textarea, [contenteditable]:not([contenteditable="false"])') && deletion) {
    event.preventDefault(); undoDeletion();
  }
});
$('completed-toggle').addEventListener('click', () => {
  if (exiting || !finishEditor()) return;
  const expanded = !settings.completed_expanded;
  if (settingsQueue.ready) changeSettings({ completed_expanded: expanded });
  else {
    // Viewing valid tasks must not depend on repairing an unrelated settings file.
    // This is temporary presentation state only; never save defaults on failure.
    settings = { ...settings, completed_expanded: expanded }; applySettings();
  }
});
$('btn-collapse').addEventListener('click', event => {
  event.stopPropagation(); changeSettings({ collapsed: !settings.collapsed });
});
// Capture the collapsed-strip gesture before hidden controls can also handle it.
let stripGesture = false;
document.addEventListener('pointerdown', event => {
  stripGesture = false;
  if (document.body.classList.contains('collapsed')) {
    event.preventDefault(); event.stopImmediatePropagation(); stripGesture = true;
    changeSettings({ collapsed: false });
  }
}, true);
document.addEventListener('click', event => {
  if (stripGesture || document.body.classList.contains('collapsed')) {
    event.preventDefault(); event.stopImmediatePropagation(); stripGesture = false;
    if (settings.collapsed) changeSettings({ collapsed: false });
    if (canEdit()) inputEl.focus();
  }
}, true);

async function preserveAndReload(queue) {
  if (exiting || !queue.error || !finishEditor()) return;
  endDrag(); exiting = true; exitError = ''; updateStatus();
  try {
    await queue.running;
    const current = snapshot(queue === todoQueue ? todos : settings);
    const result = await invoke(`preserve_and_reload_${queue.argument}`, { [queue.argument]: current });
    if (!result || typeof result.preserved_path !== 'string') throw new Error('重读响应无效');
    const value = result.error ? null : (queue === todoQueue ? validateTodos(result.value) : validateSettings(result.value));
    // The backend has synced an independent archive BEFORE replacing any state.
    // On archive failure none of these queue resets run: edits remain retryable.
    queue.latest = null; queue.revision = queue.saved = 0; queue.error = '';
    queue.loadError = result.error || ''; queue.ready = !result.error;
    recoveryNotice = `未保存改动已另存：${result.preserved_path}`;
    if (queue === todoQueue) {
      deletion = null;
      if (queue.ready) todos = value;
      render();
    } else if (queue.ready) { settings = value; applySettings(); }
  } catch (error) { queue.error = `未完成保留并重读：${message(error)}`; }
  finally { exiting = false; updateStatus(); }
}

async function quit() {
  if (exiting) return;
  if (composing.has(inputEl) || !finishEditor()) {
    exitError = '请先完成中文输入，再退出'; updateStatus(); return;
  }
  endDrag();
  exiting = true; exitError = ''; ctxMenu.classList.add('hidden'); updateStatus();
  try {
    await Promise.all(queues.map(queue => queue.flush()));
    await invoke('quit_app');
  } catch (error) {
    exiting = false;
    exitError = `未退出：${message(error)}。请重试后再退出。`;
    updateStatus();
  }
}
$('btn-quit').addEventListener('click', quit);
// Native close/Alt+F4 is prevented by the backend until this queue flush finishes.
if (window.__TAURI__.event?.listen) {
  window.__TAURI__.event.listen('sideline-close-requested', quit).catch(error => {
    exitError = `关闭保护初始化失败：${message(error)}`; updateStatus();
  });
}
$('todos-retry-load').addEventListener('click', () => loadTodos());
$('todos-recover').addEventListener('click', () => loadTodos(true));
$('settings-retry-load').addEventListener('click', loadSettings);
queues.forEach(queue => $(`${queue.argument}-preserve-reload`).addEventListener('click', () => preserveAndReload(queue)));
queues.forEach(queue => $(`${queue.argument}-retry-save`).addEventListener('click', () => {
  exitError = ''; queue.retry();
}));
window.addEventListener('beforeunload', event => {
  if (queues.some(q => q.saved < q.revision || q.error) || activeEditor) {
    event.preventDefault(); event.returnValue = '';
  }
});
window.addEventListener('contextmenu', event => {
  // Keep the platform's text editing / IME / undo menu inside editors.
  if (event.target.closest('input, textarea, [contenteditable]')) return;
  event.preventDefault();
  if (exiting || document.body.classList.contains('collapsed')) return;
  const li = event.target.closest('li.todo');
  if (li) {
    const todo = todos[Number(li.dataset.index)];
    if (!todo) return;
    const line = event.target.closest('.note-line');
    if (line) beginNoteEdit(todo, notesOf(todo)[Number(line.dataset.ni)]);
    else beginTextEdit(todo);
    return;
  }
  ctxMenu.classList.remove('hidden');
  ctxMenu.style.left = Math.max(4, Math.min(event.clientX, window.innerWidth - ctxMenu.offsetWidth - 4)) + 'px';
  ctxMenu.style.top = Math.max(4, Math.min(event.clientY, window.innerHeight - ctxMenu.offsetHeight - 4)) + 'px';
});
window.addEventListener('click', () => ctxMenu.classList.add('hidden'));
ctxMenu.querySelectorAll('.ctx-item').forEach(item => item.addEventListener('click', () => {
  ctxMenu.classList.add('hidden');
  const action = item.dataset.action;
  if (action === 'left' || action === 'right') changeSettings({ side: action });
  else if (action === 'ontop') changeSettings({ ontop: !settings.ontop });
  else if (action === 'quit') quit();
}));
loadTodos();
loadSettings();
