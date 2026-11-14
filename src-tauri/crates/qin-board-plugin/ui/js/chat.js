/* qin-board · chat: the conversation page.
 *  - createModel(): pure reducer from history + stream events to display items
 *    (runs under node).
 *  - createChat(): view + session lifecycle, grouped activity timeline, folded
 *    structured data cards, `/` slash-command & skill menu, composer, todo panel.
 * Classic script; exports QB.chat. */
(function (root) {
  'use strict';
  const QB = (root.QB = root.QB || {});
  const R = () => QB.render;
  const S = () => QB.slash;

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

  function extractBlocks(role, text) {
    if (QB.slash && QB.slash.extractMessageBlocks) return QB.slash.extractMessageBlocks(role, text);
    if (!text || !text.trim()) return [];
    if (role === 'user') return [{ kind: 'user', text, pending: false }];
    if (role === 'assistant') return [{ kind: 'assistant', text, streaming: false }];
    return [{ kind: 'notice', text: text.trim(), tone: 'muted' }];
  }

  // ======================= pure model =======================
  function createModel() {
    let seq = 0;
    const s = {
      items: [], busy: false, permissions: [], status: null, statusDetail: null,
      phase: null, bgTask: null, model: null, effort: null, title: null, rev: 0
    };
    let curText = null;
    let curReason = null;
    let lastLiveTool = null;
    let pendingInputDelta = '';
    let tools = new Map();
    let liveTail = [];

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
          status: 'pending', open: false, fullOutput: false
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
        if (m.role === 'tool') {
          const data = m.tool_data || {};
          const parsed = splitToolTiming(text);
          const t = toolItem(data.id ? String(data.id) : null, data.name || 'tool', false);
          if (typeof data.input === 'string' && data.input) t.input = data.input;
          t.output = parsed.body;
          if (parsed.duration) t.duration = parsed.duration;
          t.status = 'done';
          return;
        }

        const blocks = extractBlocks(m.role, text);
        blocks.forEach((blk) => add(blk, false));

        if (m.role === 'assistant') {
          const calls = Array.isArray(m.tool_calls) ? m.tool_calls : [];
          calls.forEach((raw) => {
            const ref = String(raw == null ? '' : raw).trim();
            if (!ref) return;
            if (toolMsgIds.has(ref)) {
              const t = toolItem(ref, null, false);
              t.status = 'done';
            } else if (idx > lastToolIdx && !keep.some((it) => it.kind === 'tool')) {
              const t = toolItem(null, ref, false);
              t.status = s.busy ? 'running' : 'done';
            }
          });
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

    function clearItems() {
      endTurn();
      s.items = [];
      tools = new Map();
      liveTail = [];
      s.rev += 1;
    }

    function addUser(text, opts) {
      const soft = !!(opts && opts.soft);
      if (!soft) { endText(); endReason(); lastLiveTool = null; pendingInputDelta = ''; }
      s.busy = true;
      return add({ kind: 'user', text, pending: true, soft }, true);
    }

    function notice(text, tone) {
      endText();
      const blocks = extractBlocks('system', String(text == null ? '' : text));
      if (blocks.length === 1 && blocks[0].kind === 'data') return add(blocks[0], true);
      return add({ kind: 'notice', text: String(text == null ? '' : text), tone: tone || 'info' }, true);
    }

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
            const blocks = extractBlocks('user', content);
            blocks.forEach((blk) => {
              if (blk.kind === 'user') { blk.soft = true; blk.role = ev.display_role || null; }
              add(blk, true);
            });
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
        case 'model_changed':
        case 'runtime_info': {
          let changed = false;
          if (ev.model) { s.model = String(ev.model); changed = true; }
          if (ev.reasoning_effort !== undefined) { s.effort = ev.reasoning_effort || null; changed = true; }
          if (ev.effort !== undefined) { s.effort = ev.effort || null; changed = true; }
          if (changed) s.rev += 1;
          return changed;
        }
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

    return { state: s, apply, loadHistory, clearItems, addUser, notice, resolvePermission, endTurn };
  }

  // ======================= view helpers =======================
  function prettyJson(text) {
    if (typeof text !== 'string' || !text.trim()) return '';
    try { return JSON.stringify(JSON.parse(text), null, 2); } catch (_) { return text; }
  }

  function prettyMaybeJson(text) {
    if (typeof text !== 'string') return '';
    const t = text.trim();
    if ((t.startsWith('{') && t.endsWith('}')) || (t.startsWith('[') && t.endsWith(']'))) {
      try { return JSON.stringify(JSON.parse(t), null, 2); } catch (_) { return text; }
    }
    return text;
  }

  function compactArg(val, key) {
    const line = String(val).trim().split('\n')[0].trim();
    if ((key === 'file_path' || key === 'path') && line.length > 54 && line.startsWith('/')) {
      const parts = line.split('/').filter(Boolean);
      if (parts.length >= 3) return '…/' + parts.slice(-2).join('/');
    }
    return line;
  }

  function parseBatchSubCalls(input) {
    if (typeof input !== 'string' || !input) return [];
    try {
      const obj = JSON.parse(input);
      if (obj && Array.isArray(obj.tool_calls)) return obj.tool_calls.filter((c) => c && typeof c === 'object');
    } catch (_) { /* partial JSON */ }
    return [];
  }

  function toolSummary(input) {
    if (typeof input !== 'string' || !input) return '';
    let obj = null;
    try { obj = JSON.parse(input); } catch (_) { obj = null; }
    if (obj && typeof obj === 'object') {
      if (Array.isArray(obj.tool_calls) && obj.tool_calls.length) {
        const names = obj.tool_calls.map((c) => c && c.tool).filter(Boolean);
        const uniq = Array.from(new Set(names)).slice(0, 3).join(', ');
        const prefix = typeof obj.intent === 'string' && obj.intent.trim() ? obj.intent.trim() + ' · ' : '';
        return prefix + '并行 ' + obj.tool_calls.length + ' 项' + (uniq ? ' (' + uniq + ')' : '');
      }
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
    let totalOps = 0;
    tools.forEach((t) => {
      if (t.name === 'batch') {
        const subs = parseBatchSubCalls(t.input);
        if (subs.length) {
          subs.forEach((sc) => {
            const sn = sc.tool || 'tool';
            counts.set(sn, (counts.get(sn) || 0) + 1);
            totalOps += 1;
          });
          return;
        }
      }
      const n = t.name || 'tool';
      counts.set(n, (counts.get(n) || 0) + 1);
      totalOps += 1;
    });
    const top = Array.from(counts.entries())
      .sort((a, b) => b[1] - a[1])
      .slice(0, 4)
      .map(([n, c]) => c > 1 ? (n + ' ×' + c) : n)
      .join(' · ');
    const parts = [];
    if (reasons.length) parts.push('思考 ' + reasons.length + ' 次');
    if (top) parts.push(top);
    const stepCount = totalOps > tools.length ? (tools.length + ' 步工具（共 ' + totalOps + ' 项）') : (tools.length + ' 步工具');
    return {
      tone: errors ? 'fail' : 'done',
      title: tools.length ? ('已执行 ' + stepCount) : '已完成思考',
      sub: parts.join(' · '),
      badge: errors ? (errors + ' 失败') : '',
      errors
    };
  }

  // ======================= chat view =======================
  function createChat(els, hooks) {
    const { el, dot, renderMarkdown, basename, stateInfo, truncate, message, shortId } = R();
    const api = hooks.api;
    let model = createModel();
    let meta = null;
    let gen = 0;
    let off = null;
    let link = 'idle';
    let linkError = null;
    let historyLoaded = false;
    let todos = null;
    let todoError = null;
    let follow = true;
    let raf = 0;
    const nodes = new Map();
    const groups = new Map();
    const groupOpen = new Map();

    // Slash menu state
    let slashCatalog = { models: [], currentModel: null, currentEffort: null, skills: [], commands: [] };
    let slashIndex = 0;
    let slashDismissed = false;
    let slashFlat = []; // [{item, node}]

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
          el('span', { class: 'fold-title', text: label }),
          item.text ? el('span', { class: 'fold-sub', text: item.text.split('\n')[0].trim() }) : null),
        el('div', { class: 'fold-body reasoning-text', text: item.text }));
      d.addEventListener('toggle', () => { item.open = d.open; });
      return d;
    }
    function expandablePre(rawText, isErr) {
      const formatted = prettyMaybeJson(rawText);
      const clipped = formatted.length > TOOL_OUTPUT_MAX;
      let expanded = false;
      const pre = el('pre', { class: 'io' + (isErr ? ' is-error' : ''), text: clipped ? formatted.slice(0, TOOL_OUTPUT_MAX) : formatted });
      if (!clipped) return [pre];
      const moreBtn = el('button', {
        type: 'button',
        class: 'io-more',
        text: '展开完整内容（共 ' + formatted.length + ' 字符）',
        on: {
          click: () => {
            expanded = !expanded;
            pre.textContent = expanded ? formatted : formatted.slice(0, TOOL_OUTPUT_MAX);
            moreBtn.textContent = expanded ? '收起长文本' : ('展开完整内容（共 ' + formatted.length + ' 字符）');
          }
        }
      });
      return [pre, moreBtn];
    }
    function toolNode(item) {
      const summary = toolSummary(item.input);
      const out = item.error || item.output;
      const body = [];
      if (item.name === 'batch') {
        const subs = parseBatchSubCalls(item.input);
        if (subs.length) {
          body.push(el('div', { class: 'io-label', text: '并行子调用 (' + subs.length + ')' }),
            el('ol', { class: 'batch-subs' }, subs.map((sc) => {
              const subSum = sc.intent || sc.file_path || sc.path || sc.command || sc.query || '';
              return el('li', null,
                el('code', { class: 'md-icode', text: sc.tool || 'tool' }),
                subSum ? el('span', { text: ' ' + subSum }) : null);
            })));
        }
      }
      const input = prettyJson(item.input);
      if (input) body.push(el('div', { class: 'io-label', text: '参数' }), ...expandablePre(input, false));
      if (out != null && out !== '') {
        body.push(el('div', { class: 'io-label', text: item.error ? '错误' : '结果' }), ...expandablePre(out, !!item.error));
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
    function dataNode(item) {
      const body = [];
      if (item.meta) body.push(el('div', { class: 'data-meta', text: item.meta }));
      if (item.detail) body.push(...expandablePre(item.detail, false));
      const d = el('details', { class: 'fold data', open: !!item.open, dataset: { subtype: item.subtype || 'system' } },
        el('summary', null,
          dot('idle'),
          el('span', { class: 'fold-title', text: item.label || '上下文' }),
          item.summary ? el('span', { class: 'fold-sub', title: item.summary, text: item.summary }) : null,
          item.badge ? el('span', { class: 'fold-state', text: item.badge }) : null),
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
        case 'data': return dataNode(item);
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

    function setAllFolded(openState) {
      model.state.items.forEach((it) => {
        if (it.kind === 'tool' || it.kind === 'reasoning' || it.kind === 'data') {
          it.open = openState;
          it.rev = (it.rev || 0) + 1;
        }
      });
      groups.forEach((g, key) => {
        groupOpen.set(key, openState);
        g.node.open = openState;
      });
      els.list.querySelectorAll('details.fold').forEach((d) => { d.open = openState; });
      schedule();
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
        else placeholder = '输入消息或输入 / 调用斜线命令与技能';
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

    function isBuiltinSlashInput(value) {
      if (!QB.slash) return false;
      const q = QB.slash.parseSlashQuery(value);
      return !!(q && QB.slash.findBuiltinCommand(q.cmd));
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
      if (els.slashBtn) els.slashBtn.disabled = !canSend;
      els.input.placeholder = canSend
        ? (st.busy ? '补充指令（Enter 插入）或输入 / 调用命令…' : '输入消息，或输入 / 调用命令与技能…')
        : (link === 'opening' ? '正在连接会话…' : '会话未连接');
      els.send.disabled = !canSend || !els.input.value.trim();
      els.send.textContent = isBuiltinSlashInput(els.input.value) ? '执行' : (st.busy ? '追加' : '发送');
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

    // ---------- slash menu ----------
    function closeSlashMenu() {
      slashFlat = [];
      if (els.slashMenu) {
        els.slashMenu.hidden = true;
        els.slashMenu.replaceChildren();
      }
    }

    function setInputValue(next) {
      els.input.value = next;
      autosize();
      els.send.disabled = link !== 'attached' || !els.input.value.trim();
      els.send.textContent = isBuiltinSlashInput(els.input.value) ? '执行' : (model.state.busy ? '追加' : '发送');
      els.input.focus();
      try { els.input.setSelectionRange(next.length, next.length); } catch (_) { /* jsdom */ }
    }

    function highlightSlash(nextIdx) {
      if (!slashFlat.length) return;
      slashIndex = ((nextIdx % slashFlat.length) + slashFlat.length) % slashFlat.length;
      slashFlat.forEach((entry, idx) => {
        const active = idx === slashIndex;
        entry.node.classList.toggle('is-active', active);
        entry.node.setAttribute('aria-selected', active ? 'true' : 'false');
        if (active && entry.node.scrollIntoView) {
          try { entry.node.scrollIntoView({ block: 'nearest' }); } catch (_) { /* noop */ }
        }
      });
    }

    function drawSlashMenu() {
      if (!els.slashMenu || !QB.slash || link !== 'attached') { closeSlashMenu(); return; }
      if (slashDismissed) { closeSlashMenu(); return; }
      const q = S().parseSlashQuery(els.input.value);
      if (!q) { closeSlashMenu(); return; }
      const ctx = {
        models: slashCatalog.models,
        currentModel: model.state.model || slashCatalog.currentModel || (meta && meta.model) || '',
        currentEffort: model.state.effort || slashCatalog.currentEffort || '',
        skills: slashCatalog.skills,
        commands: slashCatalog.commands
      };
      const sections = S().buildSlashSections(q, ctx);
      if (!sections.length) { closeSlashMenu(); return; }

      slashFlat = [];
      const frag = [];
      sections.forEach((sec) => {
        frag.push(el('div', { class: 'slash-sec', text: sec.heading }));
        sec.items.forEach((item) => {
          const idx = slashFlat.length;
          const row = el('button', {
            type: 'button',
            class: 'slash-item',
            role: 'option',
            'aria-selected': 'false',
            dataset: { kind: item.kind },
            on: {
              mouseenter: () => highlightSlash(idx),
              mousedown: (e) => e.preventDefault(), // keep textarea focused
              click: () => chooseSlashItem(item, false)
            }
          },
            el('span', { class: 'slash-name', text: item.label }),
            item.desc ? el('span', { class: 'slash-desc', text: item.desc }) : null,
            item.badge ? el('span', { class: 'slash-badge', text: item.badge }) : null
          );
          slashFlat.push({ item, node: row });
          frag.push(row);
        });
      });

      els.slashMenu.replaceChildren(...frag);
      els.slashMenu.hidden = false;
      highlightSlash(Math.min(slashIndex, slashFlat.length - 1));
    }

    function chooseSlashItem(item, viaTab) {
      if (!item || !meta) return;
      const id = meta.session_id;
      if (item.kind === 'model') {
        slashDismissed = true;
        setInputValue('');
        closeSlashMenu();
        void runModelSwitch(id, item.value);
        return;
      }
      if (item.kind === 'effort') {
        slashDismissed = true;
        setInputValue('');
        closeSlashMenu();
        void runEffortSwitch(id, item.value);
        return;
      }
      if (item.kind === 'work') {
        slashDismissed = true;
        setInputValue(item.value);
        closeSlashMenu();
        if (!viaTab) submit();
        return;
      }
      if (item.kind === 'command') {
        if (item.takesArg || viaTab) {
          slashDismissed = false;
          slashIndex = 0;
          setInputValue('/' + item.name + (item.takesArg ? ' ' : ''));
          drawSlashMenu();
          return;
        }
        slashDismissed = true;
        setInputValue('/' + item.name);
        closeSlashMenu();
        submit();
        return;
      }
      if (item.kind === 'skill') {
        const target = '/' + item.name;
        const cur = els.input.value.trim();
        if (!viaTab && cur === target) {
          slashDismissed = true;
          closeSlashMenu();
          submit();
          return;
        }
        slashDismissed = true;
        setInputValue(target + ' ');
        closeSlashMenu();
      }
    }

    async function runModelSwitch(sessionId, modelName) {
      try {
        await api.setModel(sessionId, modelName);
        if (!meta || meta.session_id !== sessionId) return;
        model.state.model = modelName;
        slashCatalog.currentModel = modelName;
        model.notice('已切换模型：' + modelName, 'info');
        schedule();
      } catch (error) {
        hooks.notify('切换模型失败：' + message(error));
      }
    }

    async function runEffortSwitch(sessionId, effort) {
      try {
        await api.setEffort(sessionId, effort);
        if (!meta || meta.session_id !== sessionId) return;
        model.state.effort = effort;
        slashCatalog.currentEffort = effort;
        model.notice('已设置推理强度：' + effort, 'info');
        schedule();
      } catch (error) {
        hooks.notify('设置推理强度失败：' + message(error));
      }
    }

    async function executeBuiltinSlash(q) {
      if (!meta || !QB.slash) return false;
      const b = S().findBuiltinCommand(q.cmd);
      if (!b) return false;
      const id = meta.session_id;
      const arg = q.args.trim();

      // Commands that need an argument when invoked bare open their picker instead of failing
      if (b.name === 'model' && !arg) {
        slashDismissed = false;
        slashIndex = 0;
        setInputValue('/model ');
        drawSlashMenu();
        if (!slashCatalog.models.length && api.listModels) {
          api.listModels(id).then((res) => {
            if (!meta || meta.session_id !== id) return;
            slashCatalog.models = res.models;
            if (res.current) slashCatalog.currentModel = res.current;
            drawSlashMenu();
          }).catch(() => {});
        }
        return true;
      }
      if (b.name === 'effort' && !arg) {
        slashDismissed = false;
        slashIndex = 0;
        setInputValue('/effort ');
        drawSlashMenu();
        return true;
      }
      if (b.name === 'work') {
        // /work is natively executed by qin-code daemon's human_auto_alias via send_message
        return false;
      }

      setInputValue('');
      closeSlashMenu();

      try {
        switch (b.name) {
          case 'compact':
            model.notice('正在压缩上下文…', 'muted');
            schedule();
            await api.compact(id);
            if (meta && meta.session_id === id) {
              const msgs = await api.history(id);
              if (meta && meta.session_id === id) { model.loadHistory(msgs, { keepLive: true }); schedule(); }
            }
            break;
          case 'clear':
            await api.clearSession(id);
            if (meta && meta.session_id === id) {
              model.clearItems();
              nodes.clear();
              groups.clear();
              groupOpen.clear();
              model.notice('会话历史已清空', 'muted');
              schedule();
            }
            break;
          case 'stop':
            stop();
            break;
          case 'model':
            await runModelSwitch(id, arg);
            break;
          case 'effort':
            await runEffortSwitch(id, arg.toLowerCase());
            break;
          case 'rename':
            await api.renameSession(id, arg || null);
            if (meta && meta.session_id === id) {
              model.state.title = arg || null;
              meta.title = arg || null;
              model.notice(arg ? ('会话已重命名为：' + arg) : '已恢复默认会话标题', 'muted');
              schedule();
            }
            break;
          case 'rewind': {
            const idx = Math.max(1, parseInt(arg, 10) || Math.max(1, model.state.items.filter((x) => x.kind === 'user').length));
            await api.rewind(id, idx);
            if (meta && meta.session_id === id) {
              const msgs = await api.history(id);
              if (meta && meta.session_id === id) { model.loadHistory(msgs, { keepLive: false }); model.notice('已回退至第 ' + idx + ' 条消息（可用 /undo 撤销）', 'muted'); schedule(); }
            }
            break;
          }
          case 'undo':
            await api.rewindUndo(id);
            if (meta && meta.session_id === id) {
              const msgs = await api.history(id);
              if (meta && meta.session_id === id) { model.loadHistory(msgs, { keepLive: false }); model.notice('已恢复上次被回退的历史', 'muted'); schedule(); }
            }
            break;
          case 'drop-queued':
            await api.cancelQueued(id);
            if (meta && meta.session_id === id) { model.notice('已清空排队待插入消息', 'muted'); schedule(); }
            break;
          case 'status': {
            const info = await api.runtimeInfo(id);
            if (meta && meta.session_id === id && info) {
              const parts = [
                info.provider ? ('服务商: ' + info.provider) : '',
                info.model ? ('模型: ' + info.model) : '',
                info.reasoning_effort ? ('推理强度: ' + info.reasoning_effort) : ''
              ].filter(Boolean);
              model.notice(parts.join(' · ') || '会话已连接', 'info');
              schedule();
            }
            break;
          }
          case 'todo':
            if (hooks.toggleTodo) hooks.toggleTodo();
            break;
          case 'fold':
            setAllFolded(false);
            break;
          case 'expand':
            setAllFolded(true);
            break;
          default:
            return false;
        }
      } catch (error) {
        hooks.notify('/' + b.name + ' 执行失败：' + message(error));
      }
      return true;
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

    function refreshSlashSources(my, id, workingDir) {
      if (api.listSlash) {
        api.listSlash(workingDir).then((cat) => {
          if (my !== gen) return;
          slashCatalog.skills = cat.skills || [];
          slashCatalog.commands = cat.commands || [];
          drawSlashMenu();
        }).catch(() => {});
      }
      if (api.listModels) {
        api.listModels(id).then((res) => {
          if (my !== gen) return;
          slashCatalog.models = res.models || [];
          if (res.current) slashCatalog.currentModel = res.current;
          drawSlashMenu();
        }).catch(() => {});
      }
      if (api.runtimeInfo) {
        api.runtimeInfo(id).then((info) => {
          if (my !== gen || !info) return;
          if (info.model && !model.state.model) model.state.model = info.model;
          if (info.reasoning_effort) {
            model.state.effort = info.reasoning_effort;
            slashCatalog.currentEffort = info.reasoning_effort;
          }
          schedule();
        }).catch(() => {});
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
        }).catch(() => {});
      }
      try {
        const session = await api.openSession(id, meta.working_dir || null);
        if (my !== gen) return;
        if (session && session.title && !model.state.title) model.state.title = session.title;
        if (session && typeof session.status === 'string') model.state.busy = BUSY_STATUS.test(session.status);
        link = 'attached';
        schedule();
        refreshSlashSources(my, id, meta.working_dir || null);
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
      slashCatalog = { models: [], currentModel: meta.model || null, currentEffort: null, skills: [], commands: [] };
      slashIndex = 0;
      slashDismissed = false;
      closeSlashMenu();
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
      closeSlashMenu();
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
      const q = QB.slash ? S().parseSlashQuery(text) : null;
      if (q && S().findBuiltinCommand(q.cmd)) {
        void executeBuiltinSlash(q).then((handled) => {
          if (!handled) sendChatText(text);
        });
        return;
      }
      sendChatText(text);
    }

    function sendChatText(text) {
      if (!meta || link !== 'attached') return;
      const id = meta.session_id;
      els.input.value = '';
      closeSlashMenu();
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
      if (e.isComposing || e.keyCode === 229) return;
      const menuOpen = !!(els.slashMenu && !els.slashMenu.hidden && slashFlat.length);
      if (menuOpen) {
        if (e.key === 'ArrowDown') { e.preventDefault(); highlightSlash(slashIndex + 1); return; }
        if (e.key === 'ArrowUp') { e.preventDefault(); highlightSlash(slashIndex - 1); return; }
        if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); slashDismissed = true; closeSlashMenu(); return; }
        if (e.key === 'Tab') { e.preventDefault(); chooseSlashItem(slashFlat[slashIndex].item, true); return; }
        if (e.key === 'Enter' && !e.shiftKey) {
          const picked = slashFlat[slashIndex] && slashFlat[slashIndex].item;
          const q = QB.slash ? S().parseSlashQuery(els.input.value) : null;
          // If user typed arguments manually (e.g. `/rename foo` or `/skill-x do bar`) and menu has no picker sub-items, submit directly;
          // if menu is showing picker items (models/efforts/work/commands/skills), choose the highlighted item.
          if (picked && (!q || !q.hasSpace || picked.kind === 'model' || picked.kind === 'effort' || picked.kind === 'work')) {
            e.preventDefault();
            chooseSlashItem(picked, false);
            return;
          }
        }
      }
      if (e.key === 'Enter' && !e.shiftKey) {
        e.preventDefault();
        submit();
      }
    });

    els.input.addEventListener('input', () => {
      autosize();
      slashDismissed = false;
      slashIndex = 0;
      drawSlashMenu();
      els.send.disabled = link !== 'attached' || !els.input.value.trim();
      els.send.textContent = isBuiltinSlashInput(els.input.value) ? '执行' : (model.state.busy ? '追加' : '发送');
    });

    if (els.slashBtn) {
      els.slashBtn.addEventListener('click', () => {
        if (link !== 'attached') return;
        if (els.slashMenu && !els.slashMenu.hidden && els.input.value === '/') {
          slashDismissed = true;
          setInputValue('');
          closeSlashMenu();
          return;
        }
        slashDismissed = false;
        slashIndex = 0;
        const cur = els.input.value;
        setInputValue(cur.trim().startsWith('/') ? cur : '/' + cur.replace(/^\s+/, ''));
        drawSlashMenu();
      });
    }

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
      lost() { if (!meta) return; gen++; closeSlashMenu(); link = 'lost'; schedule(); },
      chanClosed(reason) { if (!meta || link === 'lost') return; gen++; closeSlashMenu(); link = 'error'; linkError = '会话连接已关闭' + (reason ? '：' + reason : ''); schedule(); },
      updateMeta(session) { if (meta && session && session.session_id === meta.session_id) { meta = Object.assign({}, meta, session); schedule(); } },
      get session() { return meta; },
      get busy() { return model.state.busy; }
    };
  }

  QB.chat = { createModel, createChat, toolSummary, prettyJson, splitToolTiming };
})(typeof window !== 'undefined' ? window : globalThis);
