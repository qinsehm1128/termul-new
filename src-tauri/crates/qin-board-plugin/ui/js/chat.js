/* qin-board · chat: the conversation page.
 *  - createModel(): pure reducer from history + stream events to display items
 *    (runs under node).
 *  - createChat(): view + session lifecycle (peek → open s:<id> → attach →
 *    get_history), grouped activity timeline, composer, permission banner, todo panel.
 * Classic script; exports QB.chat. */
(function (root) {
  'use strict';
  const QB = (root.QB = root.QB || {});
  const R = () => QB.render;

  const TOOL_OUTPUT_MAX = 4000;
  const BUSY_STATUS = /generat|stream|tool|think|running|busy|working|processing/i;
  const IDLE_STATUS = /^(idle|ready|done|completed|stopped|failed|error|closed|attached)$/i;
  const TIMING_HEADER = /^\[tool timing:[^\]]*?\bduration=([^\s\]]+)\]\s*/;
  const PARTIAL_KEYS = ['command', 'file_path', 'path', 'pattern', 'query', 'url', 'intent', 'name', 'description', 'prompt'];

  function splitToolTiming(raw) {
    if (typeof raw !== 'string' || !raw) return { duration: null, body: '' };
    const m = TIMING_HEADER.exec(raw);
    if (!m) return { duration: null, body: raw };
    return { duration: m[1] || null, body: raw.slice(m[0].length) };
  }

  function translatePhase(phase) {
    if (typeof phase !== 'string' || !phase.trim()) return null;
    const p = phase.trim().toLowerCase();
    if (p === 'streaming') return null;
    if (p === 'connecting') return '连接模型…';
    if (p === 'sending request') return '发送请求…';
    if (p === 'waiting for response' || p === 'waiting') return '等待响应…';
    if (p.startsWith('retrying')) return '重试中 ' + phase.trim().replace(/^retrying\s*/i, '');
    return phase.trim();
  }

  // ======================= pure model =======================
  function createModel() {
    let seq = 0;
    const s = {
      items: [], busy: false, permissions: [], status: null, statusDetail: null,
      phase: null, bgTask: null, model: null, title: null, rev: 0
    };
    let curText = null;
    let curReason = null;
    let lastLiveTool = null;
    let pendingInputDelta = '';
    let tools = new Map();
    let liveTail = []; // items created by live events during the current turn

    const touch = (item) => { item.rev = (item.rev || 0) + 1; s.rev += 1; return item; };
    function add(item, live) {
      item.id = ++seq;
      item.rev = 1;
      s.items.push(item);
      if (live) liveTail.push(item);
      s.rev += 1;
      return item;
    }
    function endText() { if (curText) { curText.streaming = false; touch(curText); curText = null; } }
    function endReason(secs) {
      if (!curReason) return;
      curReason.done = true;
      if (Number.isFinite(secs)) curReason.secs = secs;
      touch(curReason);
      curReason = null;
    }
    function endTurn() {
      endText();
      endReason();
      tools.forEach((t) => { if (t.status === 'pending' || t.status === 'running') { t.status = 'stopped'; touch(t); } });
      s.items.forEach((it) => { if (it.kind === 'user' && it.pending) { it.pending = false; touch(it); } });
      s.busy = false;
      s.phase = null;
      s.statusDetail = null;
      s.bgTask = null;
      lastLiveTool = null;
      pendingInputDelta = '';
      liveTail = [];
      s.rev += 1;
    }
    function rollbackLiveAttempt() {
      if (liveTail.length) {
        const drop = new Set(liveTail.filter((it) => it.kind !== 'user'));
        s.items = s.items.filter((it) => !drop.has(it));
        liveTail = liveTail.filter((it) => !drop.has(it));
        tools.forEach((val, key) => { if (drop.has(val)) tools.delete(key); });
      }
      curText = null;
      curReason = null;
      lastLiveTool = null;
      pendingInputDelta = '';
      s.rev += 1;
    }
    function toolItem(callId, name, live) {
      const key = typeof callId === 'string' && callId ? callId : null;
      let t = key ? tools.get(key) : null;
      if (!t) {
        t = add({
          kind: 'tool', callId: key, name: name || '工具',
          input: '', output: null, error: null, duration: null,
          status: 'pending', open: false
        }, live);
        if (key) tools.set(key, t);
      } else if (name && (t.name === '工具' || t.name !== name)) {
        t.name = name;
        touch(t);
      }
      return t;
    }

    /** Replace items with a transcript. keepLive keeps the in-flight turn's live items. */
    function loadHistory(messages, opts) {
      const keep = opts && opts.keepLive && s.busy ? liveTail.slice() : [];
      const list = Array.isArray(messages) ? messages : [];
      const toolMsgIds = new Set();
      let lastToolIdx = -1;
      list.forEach((m, idx) => {
        if (m && m.role === 'tool') {
          lastToolIdx = idx;
          if (m.tool_data && m.tool_data.id) toolMsgIds.add(String(m.tool_data.id));
        }
      });

      s.items = [];
      tools = new Map();
      curText = null;
      curReason = null;
      lastLiveTool = null;

      list.forEach((m, idx) => {
        if (!m || typeof m !== 'object') return;
        const text = typeof m.content === 'string' ? m.content : '';
        if (m.role === 'user') {
          if (text.trim()) add({ kind: 'user', text, pending: false });
        } else if (m.role === 'system') {
          if (text.trim()) add({ kind: 'notice', text: text.trim(), tone: 'muted' });
        } else if (m.role === 'assistant') {
          if (text.trim()) add({ kind: 'assistant', text, streaming: false });
          const calls = Array.isArray(m.tool_calls) ? m.tool_calls : [];
          calls.forEach((raw) => {
            const ref = String(raw == null ? '' : raw).trim();
            if (!ref) return;
            if (toolMsgIds.has(ref)) {
              // ID-referenced tool call: the subsequent role:'tool' message fills details in place.
              const t = toolItem(ref, null, false);
              t.status = 'done';
            } else if (idx > lastToolIdx && !keep.some((it) => it.kind === 'tool')) {
              // Name-only tool_calls on an incomplete trailing assistant message without tool results yet.
              const t = toolItem(null, ref, false);
              t.status = s.busy ? 'running' : 'done';
            }
          });
        } else if (m.role === 'tool') {
          const data = m.tool_data || {};
          const parsed = splitToolTiming(text);
          const t = toolItem(data.id ? String(data.id) : null, data.name || 'tool', false);
          if (typeof data.input === 'string' && data.input) t.input = data.input;
          t.output = parsed.body;
          if (parsed.duration) t.duration = parsed.duration;
          t.status = 'done';
        }
      });

      liveTail = [];
      keep.forEach((item) => {
        if (item.kind === 'tool' && item.callId && tools.has(item.callId)) return;
        s.items.push(item);
        liveTail.push(item);
        if (item.kind === 'tool' && item.callId) tools.set(item.callId, item);
        if (item.kind === 'tool' && (item.status === 'pending' || item.status === 'running')) lastLiveTool = item;
        if (item.kind === 'assistant' && item.streaming) curText = item;
        if (item.kind === 'reasoning' && !item.done) curReason = item;
      });
      s.rev += 1;
    }

    function addUser(text, opts) {
      const soft = !!(opts && opts.soft);
      if (!soft) { endText(); endReason(); lastLiveTool = null; pendingInputDelta = ''; }
      s.busy = true;
      return add({ kind: 'user', text, pending: true, soft }, true);
    }

    function notice(text, tone) { endText(); return add({ kind: 'notice', text, tone: tone || 'info' }, true); }

    /** Apply one stream event. Returns true when something visible changed. */
    function apply(ev) {
      if (!ev || typeof ev.ev !== 'string') return false;
      switch (ev.ev) {
        case 'text_delta': {
          if (typeof ev.text !== 'string' || !ev.text) return false;
          endReason();
          s.phase = null;
          if (!curText) curText = add({ kind: 'assistant', text: '', streaming: true }, true);
          curText.text += ev.text;
          s.busy = true;
          touch(curText);
          return true;
        }
        case 'text_replace': {
          if (typeof ev.text !== 'string') return false;
          if (!curText) curText = add({ kind: 'assistant', text: '', streaming: true }, true);
          curText.text = ev.text;
          touch(curText);
          return true;
        }
        case 'reasoning_delta': {
          if (typeof ev.text !== 'string' || !ev.text) return false;
          s.phase = null;
          if (!curReason) { endText(); curReason = add({ kind: 'reasoning', text: '', done: false, secs: null, open: false }, true); }
          curReason.text += ev.text;
          s.busy = true;
          touch(curReason);
          return true;
        }
        case 'reasoning_done':
          endReason(Number(ev.duration_secs));
          return true;
        case 'tool_start': {
          endText();
          endReason();
          s.phase = null;
          const t = toolItem(ev.call_id, ev.name, true);
          if (pendingInputDelta && !t.input) { t.input = pendingInputDelta; pendingInputDelta = ''; }
          t.status = 'pending';
          lastLiveTool = t;
          s.busy = true;
          touch(t);
          return true;
        }
        case 'tool_input_delta': {
          const delta = typeof ev.delta === 'string' ? ev.delta : '';
          if (!delta) return false;
          const key = typeof ev.call_id === 'string' && ev.call_id ? ev.call_id : null;
          const t = key ? toolItem(key, null, true) : lastLiveTool;
          if (!t) {
            // Attached mid-argument-stream before tool_start: hold until tool_exec / tool_done names it.
            pendingInputDelta += delta;
            s.busy = true;
            return false;
          }
          t.input += delta;
          touch(t);
          return true;
        }
        case 'tool_exec': {
          const t = toolItem(ev.call_id, ev.name, true);
          if (pendingInputDelta && !t.input) { t.input = pendingInputDelta; pendingInputDelta = ''; }
          t.status = 'running';
          lastLiveTool = t;
          s.busy = true;
          touch(t);
          return true;
        }
        case 'tool_done': {
          const t = toolItem(ev.call_id, ev.name, true);
          if (pendingInputDelta && !t.input) { t.input = pendingInputDelta; pendingInputDelta = ''; }
          const parsed = splitToolTiming(typeof ev.output === 'string' ? ev.output : '');
          t.output = parsed.body;
          if (parsed.duration) t.duration = parsed.duration;
          t.error = ev.error ? String(ev.error) : null;
          t.status = t.error ? 'error' : 'done';
          if (lastLiveTool === t) lastLiveTool = null;
          touch(t);
          return true;
        }
        case 'retry_rollback':
          rollbackLiveAttempt();
          s.statusDetail = ev.attempt && ev.max ? ('重试中 (' + ev.attempt + '/' + ev.max + ')') : '重试中…';
          return true;
        case 'connection_phase':
          s.phase = translatePhase(ev.phase);
          s.rev += 1;
          return true;
        case 'background_progress':
          s.bgTask = ev.done ? null : { taskId: ev.task_id, label: ev.label || '', summary: ev.summary || '', percent: ev.percent };
          s.rev += 1;
          return true;
        case 'message_end':
          endText();
          endReason();
          return true;
        case 'turn_done':
          endTurn();
          return true;
        case 'interrupted':
          endTurn();
          notice('已停止', 'muted');
          return true;
        case 'message_accepted':
          s.items.forEach((it) => { if (it.kind === 'user' && it.pending && !it.soft) { it.pending = false; touch(it); } });
          return true;
        case 'soft_interrupt_injected': {
          const content = typeof ev.content === 'string' ? ev.content : '';
          const mine = s.items.find((it) => it.kind === 'user' && it.soft && it.pending && it.text === content);
          if (mine) { mine.pending = false; touch(mine); return true; }
          if (content.trim()) {
            endText();
            add({ kind: 'user', text: content, pending: false, soft: true, role: ev.display_role || null }, true);
          }
          return true;
        }
        case 'permission_request':
          if (!ev.request_id || s.permissions.some((p) => p.request_id === ev.request_id)) return false;
          s.permissions.push({ request_id: String(ev.request_id), tool_name: ev.tool_name || '', description: ev.description || '' });
          s.rev += 1;
          return true;
        case 'session_status': {
          const st = typeof ev.status === 'string' ? ev.status : '';
          s.status = st || null;
          if (IDLE_STATUS.test(st)) { if (s.busy) endTurn(); } else if (BUSY_STATUS.test(st)) s.busy = true;
          s.rev += 1;
          return true;
        }
        case 'status_detail':
          s.statusDetail = typeof ev.detail === 'string' && ev.detail ? ev.detail : null;
          s.rev += 1;
          return true;
        case 'model_info':
        case 'runtime_info':
          if (ev.model) { s.model = String(ev.model); s.rev += 1; return true; }
          return false;
        case 'session_renamed':
          s.title = ev.display_title || ev.title || s.title;
          s.rev += 1;
          return true;
        case 'compacted':
        case 'compaction':
          notice('上下文已压缩', 'muted');
          return true;
        case 'retry_after':
          notice(Math.round(Number(ev.retry_after_secs) || 0) + ' 秒后重试', 'muted');
          return true;
        case 'notification':
          notice((ev.from_name ? ev.from_name + '：' : '') + (ev.message || ''), 'info');
          return true;
        case 'error':
          if (s.busy) endTurn();
          notice(ev.message || '出错了', 'error');
          return true;
        case 'reloading':
          notice('qin-code 正在重载', 'muted');
          return true;
        default:
          return false;
      }
    }

    function resolvePermission(requestId) {
      const before = s.permissions.length;
      s.permissions = s.permissions.filter((p) => p.request_id !== requestId);
      if (s.permissions.length !== before) s.rev += 1;
    }

    return { state: s, apply, loadHistory, addUser, notice, resolvePermission, endTurn };
  }

  // ======================= view =======================
  function prettyJson(text) {
    if (typeof text !== 'string' || !text.trim()) return '';
    try { return JSON.stringify(JSON.parse(text), null, 2); } catch (_) { return text; }
  }

  function compactArg(val, key) {
    const line = String(val).trim().split('\n')[0].trim();
    if ((key === 'file_path' || key === 'path') && line.length > 54 && line.startsWith('/')) {
      const parts = line.split('/').filter(Boolean);
      if (parts.length >= 3) return '…/' + parts.slice(-2).join('/');
    }
    return line;
  }

  /** One-line summary of a tool call's input: supports both complete JSON and mid-stream partial JSON. */
  function toolSummary(input) {
    if (typeof input !== 'string' || !input) return '';
    let obj = null;
    try { obj = JSON.parse(input); } catch (_) { obj = null; }
    if (obj && typeof obj === 'object') {
      for (const key of PARTIAL_KEYS) {
        if (typeof obj[key] === 'string' && obj[key].trim()) return compactArg(obj[key], key);
      }
      if (Array.isArray(obj.todos)) return '更新 ' + obj.todos.length + ' 项任务';
      if (Array.isArray(obj.tasks)) return '派发 ' + obj.tasks.length + ' 个子任务';
      return '';
    }
    for (const key of PARTIAL_KEYS) {
      const re = new RegExp('"' + key + '"\\s*:\\s*"((?:\\\\.|[^"\\\\])*)"?');
      const m = re.exec(input);
      if (m && m[1]) {
        const unescaped = m[1].replace(/\\n/g, ' ').replace(/\\"/g, '"').replace(/\\\\/g, '\\').trim();
        if (unescaped) return compactArg(unescaped, key);
      }
    }
    return '';
  }

  const TOOL_STATUS = { pending: '准备参数', running: '执行中', done: '完成', error: '失败', stopped: '已停止' };
  const TOOL_TONE = { pending: 'wait', running: 'run', done: 'done', error: 'fail', stopped: 'idle' };
  const TODO_ORDER = { in_progress: 0, pending: 1, completed: 2, cancelled: 3 };

  function summarizeGroup(items) {
    const tools = items.filter((it) => it.kind === 'tool');
    const reasons = items.filter((it) => it.kind === 'reasoning');
    const active = items.find((it) => (it.kind === 'tool' && (it.status === 'running' || it.status === 'pending')) || (it.kind === 'reasoning' && !it.done));
    const errors = tools.filter((t) => t.status === 'error').length;
    if (active) {
      if (active.kind === 'reasoning') {
        return { tone: 'run', title: '正在思考…', sub: tools.length ? ('已完成 ' + tools.filter((t) => t.status === 'done').length + ' 步工具') : '', badge: items.length + ' 步', errors };
      }
      const sub = toolSummary(active.input);
      return {
        tone: active.status === 'running' ? 'run' : 'wait',
        title: (active.status === 'running' ? '正在执行 ' : '准备调用 ') + (active.name || '工具'),
        sub,
        badge: (tools.filter((t) => t.status === 'done').length + 1) + ' / ' + tools.length,
        errors
      };
    }
    const counts = new Map();
    tools.forEach((t) => { const n = t.name || 'tool'; counts.set(n, (counts.get(n) || 0) + 1); });
    const top = Array.from(counts.entries())
      .sort((a, b) => b[1] - a[1])
      .slice(0, 4)
      .map(([n, c]) => c > 1 ? (n + ' ×' + c) : n)
      .join(' · ');
    const parts = [];
    if (reasons.length) parts.push('思考 ' + reasons.length + ' 次');
    if (top) parts.push(top);
    return {
      tone: errors ? 'fail' : 'done',
      title: tools.length ? ('已执行 ' + tools.length + ' 步工具') : ('已完成思考'),
      sub: parts.join(' · '),
      badge: errors ? (errors + ' 失败') : '',
      errors
    };
  }

  function createChat(els, hooks) {
    const { el, dot, renderMarkdown, basename, stateInfo, truncate, message, shortId } = R();
    const api = hooks.api;
    let model = createModel();
    let meta = null;         // BoardSession (or a stub {session_id})
    let gen = 0;             // invalidates async work of a previous session
    let off = null;          // stream listener cleanup
    let link = 'idle';       // idle | opening | attached | lost | error
    let linkError = null;
    let historyLoaded = false;
    let todos = null;
    let todoError = null;
    let follow = true;
    let raf = 0;
    const nodes = new Map();     // item id -> {rev, node}
    const groups = new Map();    // group key -> {sig, node, body}
    const groupOpen = new Map(); // group key -> boolean

    // ---------- items ----------
    function userNode(item) {
      return el('div', { class: 'msg user' + (item.pending ? ' is-pending' : '') + (item.soft ? ' is-soft' : '') },
        el('div', { class: 'bubble', text: item.text }),
        item.soft ? el('div', { class: 'msg-note', text: item.pending ? '追加中…' : '已追加' }) : null);
    }
    function assistantNode(item) {
      return el('div', { class: 'msg assistant' }, renderMarkdown(item.text, { cursor: item.streaming }));
    }
    function reasoningNode(item) {
      const label = item.done ? ('思考过程' + (Number.isFinite(item.secs) && item.secs > 0 ? ' · ' + Math.round(item.secs) + 's' : '')) : '思考中…';
      const d = el('details', { class: 'fold reasoning', open: item.open },
        el('summary', null,
          dot(item.done ? 'idle' : 'run'),
          el('span', { class: 'fold-title', text: label })),
        el('div', { class: 'fold-body reasoning-text', text: item.text }));
      d.addEventListener('toggle', () => { item.open = d.open; });
      return d;
    }
    function toolNode(item) {
      const summary = toolSummary(item.input);
      const out = item.error || item.output;
      const body = [];
      const input = prettyJson(item.input);
      if (input) body.push(el('div', { class: 'io-label', text: '参数' }), el('pre', { class: 'io', text: input }));
      if (out != null && out !== '') {
        const clipped = out.length > TOOL_OUTPUT_MAX;
        body.push(el('div', { class: 'io-label', text: item.error ? '错误' : '结果' }),
          el('pre', { class: 'io' + (item.error ? ' is-error' : ''), text: clipped ? out.slice(0, TOOL_OUTPUT_MAX) : out }));
        if (clipped) body.push(el('div', { class: 'io-clip', text: '已截断，共 ' + out.length + ' 字符' }));
      }
      if (!body.length) body.push(el('div', { class: 'io-clip', text: item.status === 'done' ? '已完成（无文本输出）' : '等待工具返回…' }));
      const stateText = item.status === 'done' ? (item.duration || '') : (TOOL_STATUS[item.status] || '');
      const d = el('details', { class: 'fold tool', open: item.open, dataset: { status: item.status } },
        el('summary', null,
          dot(TOOL_TONE[item.status] || 'idle'),
          el('span', { class: 'fold-title', text: item.name || 'tool' }),
          summary ? el('span', { class: 'fold-sub', title: summary, text: summary }) : null,
          stateText ? el('span', { class: 'fold-state', text: stateText }) : null),
        el('div', { class: 'fold-body' }, body));
      d.addEventListener('toggle', () => { item.open = d.open; });
      return d;
    }
    function noticeNode(item) {
      return el('div', { class: 'notice', dataset: { tone: item.tone }, text: item.text });
    }
    function itemNode(item) {
      switch (item.kind) {
        case 'user': return userNode(item);
        case 'assistant': return assistantNode(item);
        case 'reasoning': return reasoningNode(item);
        case 'tool': return toolNode(item);
        default: return noticeNode(item);
      }
    }

    function ensureItemNode(item) {
      let cached = nodes.get(item.id);
      if (!cached || cached.rev !== item.rev) {
        const node = itemNode(item);
        if (cached && cached.node.parentNode) cached.node.parentNode.replaceChild(node, cached.node);
        cached = { rev: item.rev, node };
        nodes.set(item.id, cached);
      }
      return cached.node;
    }

    function groupNode(run) {
      const key = 'g:' + run[0].id;
      const sig = run.map((it) => it.id + ':' + it.rev).join(',');
      let g = groups.get(key);
      const info = summarizeGroup(run);
      const defaultOpen = info.errors > 0;
      const isOpen = groupOpen.has(key) ? groupOpen.get(key) : defaultOpen;
      if (!g) {
        const titleEl = el('span', { class: 'ag-title' });
        const subEl = el('span', { class: 'ag-sub' });
        const badgeEl = el('span', { class: 'ag-badge' });
        const dotWrap = el('span', { class: 'ag-dot' });
        const body = el('div', { class: 'activity-steps' });
        const details = el('details', { class: 'activity-group', open: isOpen },
          el('summary', { class: 'activity-head' }, dotWrap, titleEl, subEl, badgeEl),
          body);
        details.addEventListener('toggle', () => { groupOpen.set(key, details.open); });
        g = { sig: '', node: details, body, dotWrap, titleEl, subEl, badgeEl };
        groups.set(key, g);
      }
      if (g.sig !== sig) {
        g.sig = sig;
        g.node.dataset.tone = info.tone;
        g.dotWrap.replaceChildren(dot(info.tone));
        g.titleEl.textContent = info.title;
        g.subEl.textContent = info.sub || '';
        g.subEl.hidden = !info.sub;
        g.badgeEl.textContent = info.badge || '';
        g.badgeEl.hidden = !info.badge;
        g.badgeEl.dataset.error = info.errors ? 'true' : 'false';
        if (!groupOpen.has(key) && info.errors > 0) g.node.open = true;
        let prev = null;
        run.forEach((it) => {
          const child = ensureItemNode(it);
          const want = prev ? prev.nextSibling : g.body.firstChild;
          if (want !== child) g.body.insertBefore(child, want);
          prev = child;
        });
        while (prev && prev.nextSibling) g.body.removeChild(prev.nextSibling);
      }
      return { key, node: g.node };
    }

    function nearBottom() {
      const sc = els.scroll;
      return sc.scrollHeight - sc.scrollTop - sc.clientHeight < 56;
    }

    function drawList() {
      const st = model.state;
      const list = els.list;
      const keepItems = new Set();
      const keepGroups = new Set();
      const blocks = [];

      for (let i = 0; i < st.items.length; ) {
        const it = st.items[i];
        keepItems.add(it.id);
        if (it.kind === 'tool' || it.kind === 'reasoning') {
          const run = [it];
          i += 1;
          while (i < st.items.length && (st.items[i].kind === 'tool' || st.items[i].kind === 'reasoning')) {
            keepItems.add(st.items[i].id);
            run.push(st.items[i]);
            i += 1;
          }
          if (run.length >= 2) {
            const g = groupNode(run);
            keepGroups.add(g.key);
            blocks.push(g.node);
          } else {
            blocks.push(ensureItemNode(it));
          }
        } else {
          blocks.push(ensureItemNode(it));
          i += 1;
        }
      }

      let prev = null;
      blocks.forEach((node) => {
        const want = prev ? prev.nextSibling : list.firstChild;
        if (want !== node) list.insertBefore(node, want);
        prev = node;
      });
      while (prev && prev.nextSibling) list.removeChild(prev.nextSibling);
      if (!prev) list.replaceChildren();

      nodes.forEach((v, id) => { if (!keepItems.has(id)) { if (v.node.parentNode) v.node.parentNode.removeChild(v.node); nodes.delete(id); } });
      groups.forEach((g, key) => { if (!keepGroups.has(key)) { if (g.node.parentNode) g.node.parentNode.removeChild(g.node); groups.delete(key); } });

      let placeholder = null;
      if (!st.items.length) {
        if (!historyLoaded && (link === 'opening' || link === 'idle')) placeholder = '正在同步对话…';
        else if (link === 'error') placeholder = null;
        else placeholder = '发送消息开始对话';
      }
      els.empty.hidden = !placeholder;
      els.empty.textContent = placeholder || '';

      const lastItem = st.items[st.items.length - 1];
      const streamingText = !!(lastItem && lastItem.kind === 'assistant' && lastItem.streaming);
      if (st.busy && !streamingText) {
        const bgText = st.bgTask ? ([st.bgTask.label, st.bgTask.summary].filter(Boolean).join(' · ')) : null;
        const label = st.statusDetail || st.phase || bgText || '思考中…';
        els.thinking.hidden = false;
        els.thinking.replaceChildren(
          el('span', { class: 'thinking-dots', 'aria-hidden': 'true' }, el('i'), el('i'), el('i')),
          el('span', { text: label }));
      } else {
        els.thinking.hidden = true;
      }
    }

    function drawPermissions() {
      const perms = model.state.permissions;
      els.perms.replaceChildren(...perms.map((p) => {
        const answer = (decision, btn) => {
          btn.parentNode.querySelectorAll('button').forEach((b) => { b.disabled = true; });
          api.permission(meta.session_id, p.request_id, decision)
            .then(() => { model.resolvePermission(p.request_id); schedule(); })
            .catch((error) => {
              btn.parentNode.querySelectorAll('button').forEach((b) => { b.disabled = false; });
              hooks.notify('权限回复失败：' + message(error));
            });
        };
        const actions = el('div', { class: 'perm-actions' });
        const mk = (label, decision, cls) => {
          const b = el('button', { type: 'button', class: 'btn ' + (cls || ''), text: label });
          b.addEventListener('click', () => answer(decision, b));
          return b;
        };
        actions.append(mk('允许', 'allow', 'primary'), mk('总是允许', 'allow_always'), mk('拒绝', 'deny', 'danger'));
        return el('div', { class: 'perm', role: 'alert' },
          el('div', { class: 'perm-text' },
            el('span', { class: 'perm-tool', text: p.tool_name || '权限确认' }),
            p.description ? el('span', { class: 'perm-desc', text: truncate(p.description, 600) }) : null),
          actions);
      }));
      els.perms.hidden = perms.length === 0;
    }

    function fallbackTitle(st, m) {
      const firstUser = st.items.find((it) => it.kind === 'user' && it.text && it.text.trim());
      if (firstUser) {
        const line = firstUser.text.trim().split('\n')[0].trim();
        if (line) return truncate(line, 36);
      }
      if (m.current_task && m.current_task.trim()) return truncate(m.current_task.trim(), 36);
      if (m.kind === 'teammate') return m.friendly_name || '子代理';
      return '未命名会话';
    }

    function drawHeader() {
      const st = model.state;
      const m = meta || {};
      const title = st.title || m.title || fallbackTitle(st, m);
      els.title.textContent = title;
      els.title.title = title + '\n' + (m.session_id || '');
      els.dir.textContent = m.working_dir ? basename(m.working_dir) : '';
      els.dir.hidden = !els.dir.textContent;
      els.dir.title = m.working_dir || '';
      let stateKey = st.busy ? 'streaming' : (m.state && m.state !== 'streaming' ? m.state : 'idle');
      if (model.state.permissions.length) stateKey = 'waiting';
      const info = stateInfo(stateKey);
      let label = info.label;
      let tone = info.tone;
      if (link === 'opening') { label = '连接中'; tone = 'wait'; }
      else if (link === 'lost') { label = '已断开'; tone = 'fail'; }
      else if (link === 'error') { label = '无法打开'; tone = 'fail'; }
      els.state.dataset.tone = tone;
      els.state.replaceChildren(dot(tone), el('span', { text: label }));
      els.model.textContent = st.model || m.model || '';
      els.model.hidden = !els.model.textContent;
      const isSub = m.kind === 'teammate';
      els.sub.hidden = !isSub;
      els.sub.textContent = isSub ? ('子代理' + (m.role ? ' · ' + m.role : '')) : '';
      const parent = isSub && m.parent_session_id ? m.parent_session_id : null;
      els.parent.hidden = !parent;
      if (parent) {
        const p = hooks.findSession(parent);
        els.parent.textContent = '↑ ' + (p ? (p.title || '父会话') : '父会话 ' + shortId(parent));
      }
      const canSend = link === 'attached';
      els.input.disabled = !canSend;
      els.input.placeholder = canSend ? (st.busy ? '补充指令，Enter 立即插入当前回合…' : '输入消息，Enter 发送，Shift+Enter 换行…') : (link === 'opening' ? '正在连接会话…' : '会话未连接');
      els.send.disabled = !canSend || !els.input.value.trim();
      els.send.textContent = st.busy ? '追加' : '发送';
      els.stop.hidden = !st.busy;
      els.stop.disabled = !canSend;
      if (link === 'error' && linkError) { els.banner.hidden = false; els.banner.textContent = linkError; }
      else if (link === 'lost') { els.banner.hidden = false; els.banner.textContent = '连接已断开，正在重连…'; }
      else els.banner.hidden = true;
    }

    function drawTodos() {
      const toggleBadge = els.todoBadge;
      if (todoError && !todos) {
        els.todoList.replaceChildren(el('li', { class: 'todo-empty', text: todoError }));
        els.todoCount.textContent = '';
        if (toggleBadge) { toggleBadge.textContent = ''; toggleBadge.hidden = true; }
        return;
      }
      const items = (todos || []).slice().sort((a, b) => (TODO_ORDER[a.status] ?? 1) - (TODO_ORDER[b.status] ?? 1));
      const done = items.filter((t) => t.status === 'completed').length;
      const ratio = items.length ? (done + '/' + items.length) : '';
      els.todoCount.textContent = ratio;
      if (toggleBadge) {
        toggleBadge.textContent = ratio;
        toggleBadge.hidden = !ratio;
      }
      if (els.todoFill) {
        els.todoFill.style.width = items.length ? Math.round((done / items.length) * 100) + '%' : '0%';
      }
      if (els.todoBar) {
        els.todoBar.hidden = !items.length;
      }
      if (!items.length) {
        els.todoList.replaceChildren(el('li', { class: 'todo-empty', text: todos ? '暂无任务清单' : '加载中…' }));
        return;
      }
      els.todoList.replaceChildren(...items.map((t) => {
        const status = t.status || 'pending';
        return el('li', { class: 'todo', dataset: { status } },
          el('span', { class: 'todo-mark', 'aria-hidden': 'true', text: status === 'completed' ? '✓' : '' }),
          el('span', { class: 'todo-text' },
            el('span', { class: 'todo-content', text: t.content || '' }),
            (t.assigned_to || (t.blocked_by && t.blocked_by.length)) ? el('span', { class: 'todo-meta',
              text: [t.assigned_to ? '@' + t.assigned_to : '', t.blocked_by && t.blocked_by.length ? '等待 ' + t.blocked_by.join(', ') : ''].filter(Boolean).join(' · ') }) : null));
      }));
    }

    function draw() {
      raf = 0;
      if (!meta) return;
      const stick = follow;
      drawList();
      drawPermissions();
      drawHeader();
      if (stick) els.scroll.scrollTop = els.scroll.scrollHeight;
      els.toBottom.hidden = follow;
    }
    function schedule() {
      if (raf || !meta) return;
      raf = (root.requestAnimationFrame || ((fn) => root.setTimeout(fn, 16)))(draw);
    }

    // ---------- lifecycle ----------
    function onFrame(frame) {
      if (!meta) return;
      if (frame.session_id && frame.session_id !== meta.session_id) return;
      if (model.apply(frame)) {
        if (frame.ev === 'turn_done' || frame.ev === 'interrupted') hooks.turnEnded();
        schedule();
      }
    }

    async function attach() {
      if (!meta) return;
      const my = ++gen;
      const id = meta.session_id;
      link = 'opening';
      linkError = null;
      schedule();
      if (!hooks.isConnected()) { link = 'lost'; schedule(); return; }
      if (!historyLoaded) {
        api.peek(id, 80).then((messages) => {
          if (my !== gen || historyLoaded || !messages.length) return;
          model.loadHistory(messages, { keepLive: true });
          schedule();
        }).catch(() => { /* attach + get_history will fill in */ });
      }
      try {
        const session = await api.openSession(id, meta.working_dir || null);
        if (my !== gen) return;
        if (session && session.title && !model.state.title) model.state.title = session.title;
        if (session && typeof session.status === 'string') model.state.busy = BUSY_STATUS.test(session.status);
        link = 'attached';
        schedule();
        const messages = await api.history(id);
        if (my !== gen) return;
        model.loadHistory(messages, { keepLive: true });
        historyLoaded = true;
        follow = true;
        schedule();
      } catch (error) {
        if (my !== gen) return;
        if (link === 'attached') { hooks.notify('读取历史失败：' + message(error)); historyLoaded = true; schedule(); return; }
        link = error && error.code === 'not_connected' ? 'lost' : 'error';
        linkError = '无法打开会话：' + message(error);
        schedule();
      }
    }

    function enter(session) {
      leave({ silent: true });
      meta = Object.assign({}, session);
      model = createModel();
      if (meta.state === 'streaming' || meta.state === 'waiting') model.state.busy = true;
      nodes.clear();
      groups.clear();
      groupOpen.clear();
      els.list.replaceChildren();
      els.input.value = '';
      autosize();
      todos = null;
      todoError = null;
      historyLoaded = false;
      follow = true;
      link = 'idle';
      off = api.onEvents(api.sessionChan(meta.session_id), onFrame);
      drawTodos();
      draw();
      void attach();
    }

    function leave() {
      if (!meta) return;
      gen++;
      if (off) { off(); off = null; }
      api.closeSession(meta.session_id);
      meta = null;
      link = 'idle';
      if (raf) { (root.cancelAnimationFrame || root.clearTimeout)(raf); raf = 0; }
    }

    async function loadTodos() {
      if (!meta) return;
      const id = meta.session_id;
      try {
        const items = await api.getTodos(id);
        if (!meta || meta.session_id !== id) return;
        todos = items;
        todoError = null;
      } catch (error) {
        if (!meta || meta.session_id !== id) return;
        todoError = QB.api.isUpgradeError(error) ? '需要升级 qin-code（API ≥ 1.3）' : '读取失败：' + message(error);
        if (!QB.api.isUpgradeError(error)) throw error;
      } finally {
        if (meta && meta.session_id === id) drawTodos();
      }
    }

    // ---------- composer ----------
    function autosize() {
      els.input.style.height = 'auto';
      const sh = els.input.scrollHeight || 0;
      els.input.style.height = Math.min(Math.max(sh, 24), 200) + 'px';
      els.input.style.overflowY = sh > 200 ? 'auto' : 'hidden';
    }

    function submit() {
      if (!meta || link !== 'attached') return;
      const text = els.input.value;
      if (!text.trim()) return;
      const id = meta.session_id;
      els.input.value = '';
      autosize();
      follow = true;
      if (model.state.busy) {
        const item = model.addUser(text, { soft: true });
        api.softInterrupt(id, text).catch((error) => {
          item.pending = false;
          item.failed = true;
          model.notice('追加失败：' + message(error), 'error');
          schedule();
        });
      } else {
        model.addUser(text);
        api.sendMessage(id, text, (error) => {
          if (!meta || meta.session_id !== id) return;
          model.endTurn();
          model.notice('发送失败：' + message(error), 'error');
          schedule();
        });
      }
      schedule();
    }

    function stop() {
      if (!meta) return;
      els.stop.disabled = true;
      api.cancel(meta.session_id).catch((error) => {
        hooks.notify('停止失败：' + message(error));
      }).finally(() => { els.stop.disabled = false; });
    }

    els.input.addEventListener('keydown', (e) => {
      if (e.key === 'Enter' && !e.shiftKey && !e.isComposing && e.keyCode !== 229) { e.preventDefault(); submit(); }
    });
    els.input.addEventListener('input', () => { autosize(); els.send.disabled = link !== 'attached' || !els.input.value.trim(); });
    els.send.addEventListener('click', submit);
    els.stop.addEventListener('click', stop);
    els.scroll.addEventListener('scroll', () => {
      const atBottom = nearBottom();
      if (atBottom !== follow) { follow = atBottom; els.toBottom.hidden = follow; }
    }, { passive: true });
    els.toBottom.addEventListener('click', () => {
      follow = true;
      els.scroll.scrollTop = els.scroll.scrollHeight;
      els.toBottom.hidden = true;
    });
    els.parent.addEventListener('click', () => {
      if (!meta || !meta.parent_session_id) return;
      hooks.open(hooks.findSession(meta.parent_session_id) || { session_id: meta.parent_session_id });
    });

    return {
      enter,
      leave,
      attach,
      loadTodos,
      reload() {
        if (!meta || link === 'opening') return;
        if (link !== 'attached') { void attach(); return; }
        const my = gen;
        api.history(meta.session_id)
          .then((m) => { if (my === gen) { model.loadHistory(m, { keepLive: true }); schedule(); } })
          .catch((e) => hooks.notify('读取历史失败：' + message(e)));
      },
      lost() { if (!meta) return; gen++; link = 'lost'; schedule(); },
      chanClosed(reason) { if (!meta || link === 'lost') return; gen++; link = 'error'; linkError = '会话连接已关闭' + (reason ? '：' + reason : ''); schedule(); },
      updateMeta(session) { if (meta && session && session.session_id === meta.session_id) { meta = Object.assign({}, meta, session); schedule(); } },
      get session() { return meta; },
      get busy() { return model.state.busy; }
    };
  }

  QB.chat = { createModel, createChat, toolSummary, prettyJson, splitToolTiming };
})(typeof window !== 'undefined' ? window : globalThis);
