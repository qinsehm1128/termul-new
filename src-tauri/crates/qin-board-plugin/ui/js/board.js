/* qin-board · board: the board page. buildModel() is pure (grouping, tree,
 * filtering, sorting) so it can be checked under node; createBoard() renders it.
 * Classic script; exports QB.board. */
(function (root) {
  'use strict';
  const QB = (root.QB = root.QB || {});
  const R = () => QB.render;

  const ACTIVE_STATES = new Set(['streaming', 'waiting']);
  const isActive = (s) => !!s && ACTIVE_STATES.has(s.state);
  const lastActive = (s) => (s && Number.isFinite(s.last_active_ms) ? s.last_active_ms : 0);
  const norm = (p) => (typeof p === 'string' ? p.replace(/[\\/]+$/, '') || p.slice(0, 1) : '');

  function byActivity(a, b) {
    const d = (isActive(b) ? 1 : 0) - (isActive(a) ? 1 : 0);
    return d || lastActive(b) - lastActive(a);
  }

  function haystack(s) {
    return [s.title, s.working_dir, s.current_task, s.friendly_name, s.role, s.model, s.session_id]
      .filter((v) => typeof v === 'string' && v).join('\n').toLowerCase();
  }

  /**
   * sessions: BoardSession[]; opts: {filter:'active'|'all', query, dir (path|null), projectPath}
   * → {dirs, groups, selected, totals:{active,total}, shown, empty:'none'|'filtered'|null}
   */
  function buildModel(sessions, opts) {
    const o = opts || {};
    const list = (Array.isArray(sessions) ? sessions : []).filter((s) => s && typeof s.session_id === 'string');
    const byId = new Map(list.map((s) => [s.session_id, s]));
    const kids = new Map();
    list.forEach((s) => {
      const parent = s.kind === 'teammate' ? s.parent_session_id : null;
      if (parent && parent !== s.session_id && byId.has(parent)) {
        if (!kids.has(parent)) kids.set(parent, []);
        kids.get(parent).push(s);
      }
    });
    const roots = list.filter((s) => !(s.kind === 'teammate' && s.parent_session_id && s.parent_session_id !== s.session_id && byId.has(s.parent_session_id)));

    const seen = new Set();
    function descend(id, depth, out) {
      const children = (kids.get(id) || []).slice().sort(byActivity);
      children.forEach((c) => {
        if (seen.has(c.session_id)) return;
        seen.add(c.session_id);
        out.push({ session: c, depth: Math.min(depth, 3) });
        descend(c.session_id, depth + 1, out);
      });
      return out;
    }

    const cards = roots.map((r) => {
      seen.add(r.session_id);
      const subs = descend(r.session_id, 1, []);
      const active = isActive(r) || subs.some((x) => isActive(x.session));
      const latest = Math.max(lastActive(r), ...subs.map((x) => lastActive(x.session)));
      return { root: r, subs, active, latest, dir: norm(r.working_dir) };
    });

    const project = norm(o.projectPath);
    const dirMap = new Map();
    cards.forEach((c) => {
      if (!dirMap.has(c.dir)) dirMap.set(c.dir, { path: c.dir, active: 0, total: 0, latest: 0, isProject: !!project && c.dir === project });
      const d = dirMap.get(c.dir);
      d.total += 1;
      if (c.active) d.active += 1;
      d.latest = Math.max(d.latest, c.latest);
    });
    const dirs = Array.from(dirMap.values()).sort((a, b) =>
      (b.isProject - a.isProject) || ((b.active > 0) - (a.active > 0)) || (b.latest - a.latest) || a.path.localeCompare(b.path));

    const selectedDir = o.dir != null && dirMap.has(norm(o.dir)) ? norm(o.dir) : null;
    const query = String(o.query || '').trim().toLowerCase();
    const terms = query ? query.split(/\s+/) : [];
    const visible = cards.filter((c) => {
      if (selectedDir != null && c.dir !== selectedDir) return false;
      if (o.filter === 'active' && !c.active) return false;
      if (!terms.length) return true;
      const text = haystack(c.root) + '\n' + c.subs.map((x) => haystack(x.session)).join('\n');
      return terms.every((t) => text.indexOf(t) !== -1);
    });
    visible.sort((a, b) => ((b.active ? 1 : 0) - (a.active ? 1 : 0)) || (b.latest - a.latest));

    const groupMap = new Map();
    visible.forEach((c) => {
      if (!groupMap.has(c.dir)) groupMap.set(c.dir, { path: c.dir, isProject: !!project && c.dir === project, cards: [] });
      groupMap.get(c.dir).cards.push(c);
    });
    const order = new Map(dirs.map((d, i) => [d.path, i]));
    const groups = Array.from(groupMap.values()).sort((a, b) => order.get(a.path) - order.get(b.path));

    return {
      dirs,
      groups,
      selected: selectedDir,
      totals: { active: cards.filter((c) => c.active).length, total: cards.length },
      shown: visible.length,
      empty: cards.length === 0 ? 'none' : visible.length === 0 ? 'filtered' : null
    };
  }

  /** The directory a project path selects by default: exact match, else the nearest enclosing/enclosed dir. */
  function projectDir(dirs, projectPath) {
    const p = norm(projectPath);
    if (!p) return null;
    const exact = dirs.find((d) => d.path === p);
    if (exact) return exact.path;
    const inside = dirs.filter((d) => d.path.startsWith(p + '/')).sort((a, b) => b.latest - a.latest)[0];
    return inside ? inside.path : null;
  }

  // ---------------- view ----------------
  function createBoard(els, hooks) {
    const { el, dot, relTime, basename, shortId, stateInfo, truncate } = R();
    let data = null;           // last list_board reply
    let lastModel = null;
    let view = { filter: 'all', query: '', dir: null, projectPath: null };
    let problem = null;        // {kind:'connecting'|'down'|'qin'|'upgrade'|'revoked'|'standalone', text}

    function dirButton(label, title, counts, selected, isProject, onClick) {
      return el('button', {
        type: 'button', class: 'dir' + (selected ? ' is-selected' : '') + (isProject ? ' is-project' : ''),
        title, 'aria-pressed': selected ? 'true' : 'false', on: { click: onClick }
      },
      el('span', { class: 'dir-name', text: label }),
      el('span', { class: 'dir-count' },
        counts.active ? el('span', { class: 'dir-active', text: String(counts.active) }) : null,
        el('span', { text: String(counts.total) })));
    }

    function renderDirs(model) {
      const nodes = [dirButton('全部', '全部目录', model.totals, model.selected == null, false, () => hooks.selectDir(null))];
      model.dirs.forEach((d) => {
        nodes.push(dirButton(d.path ? basename(d.path) : '未知目录', d.path || '没有工作目录', d, model.selected === d.path, d.isProject,
          () => hooks.selectDir(d.path)));
      });
      els.dirs.replaceChildren(...nodes);
    }

    function progress(s) {
      const total = Number(s.todo_total) || 0;
      if (total <= 0) return null;
      const done = Math.min(total, Math.max(0, Number(s.todo_done) || 0));
      return el('span', { class: 'progress', title: 'todo ' + done + '/' + total },
        el('span', { class: 'bar' }, el('span', { class: 'fill', style: { width: Math.round((done / total) * 100) + '%' } })),
        el('span', { class: 'progress-text', text: done + '/' + total }));
    }

    function subRow(item, now) {
      const s = item.session;
      const info = stateInfo(s.state);
      const name = s.friendly_name || s.title || shortId(s.session_id);
      return el('button', {
        type: 'button', class: 'sub', style: { paddingLeft: (12 + (item.depth - 1) * 16) + 'px' },
        on: { click: () => hooks.open(s) }
      },
      el('span', { class: 'sub-line' },
        dot(info.tone, info.label),
        el('span', { class: 'sub-name', text: name }),
        s.role ? el('span', { class: 'chip', text: s.role }) : null,
        el('span', { class: 'sub-state', text: info.label }),
        el('span', { class: 'sub-task', text: s.current_task || '' }),
        el('span', { class: 'time', text: relTime(s.last_active_ms, now) })),
      s.output_tail ? el('span', { class: 'tail', title: truncate(s.output_tail, 400), text: s.output_tail }) : null);
    }

    function card(c, now) {
      const s = c.root;
      const info = stateInfo(s.state);
      const title = s.title || (s.kind === 'teammate' ? (s.friendly_name || '子代理') : '未命名会话');
      const meta = [s.model, relTime(s.last_active_ms, now)].filter(Boolean).join(' · ');
      return el('article', { class: 'card' + (c.active ? ' is-active' : '') },
        el('button', { type: 'button', class: 'card-main', on: { click: () => hooks.open(s) } },
          el('span', { class: 'card-head' },
            dot(info.tone, info.label),
            el('span', { class: 'card-title', text: title, title: title + '\n' + s.session_id }),
            s.kind === 'teammate' ? el('span', { class: 'chip', text: '子代理' }) : null,
            el('span', { class: 'card-meta', text: meta })),
          (s.current_task || progress(s)) ? el('span', { class: 'card-task-row' },
            el('span', { class: 'card-task', text: s.current_task || '', title: s.current_task || null }),
            progress(s)) : null),
        c.subs.length ? el('div', { class: 'subs' }, c.subs.map((x) => subRow(x, now))) : null);
    }

    function emptyState(kind, text) {
      const restart = el('button', { type: 'button', class: 'btn', text: '重启服务', on: { click: () => hooks.restart() } });
      const retry = el('button', { type: 'button', class: 'btn ghost', text: '重试', on: { click: () => hooks.retry() } });
      switch (kind) {
        case 'down':
          return el('div', { class: 'empty' }, el('p', { class: 'empty-title', text: '连不上看板服务' }),
            text ? el('p', { class: 'empty-sub', text }) : null, el('div', { class: 'empty-actions' }, restart, retry));
        case 'qin':
          return el('div', { class: 'empty' }, el('p', { class: 'empty-title', text: 'qin-code 未运行' }),
            el('p', { class: 'empty-sub' }, '在终端里运行 ', el('code', { text: 'qin-code' })),
            text ? el('p', { class: 'empty-sub faint', text }) : null, el('div', { class: 'empty-actions' }, retry, restart));
        case 'upgrade':
          return el('div', { class: 'empty' }, el('p', { class: 'empty-title', text: '需要升级 qin-code（API ≥ 1.3）' }),
            el('p', { class: 'empty-sub', text: text || '当前版本不支持看板接口' }));
        case 'revoked':
        case 'standalone':
          return el('div', { class: 'empty' }, el('p', { class: 'empty-title', text: text }));
        case 'connecting':
          return el('div', { class: 'empty' }, el('p', { class: 'empty-title muted', text: text || '连接中…' }));
        case 'none':
          return el('div', { class: 'empty' }, el('p', { class: 'empty-title', text: '还没有会话' }),
            el('p', { class: 'empty-sub' }, '在终端里运行 ', el('code', { text: 'qin-code' })));
        default:
          return el('div', { class: 'empty' }, el('p', { class: 'empty-title muted', text: '没有匹配的会话' }));
      }
    }

    function draw() {
      const now = Date.now();
      if (!data) {
        els.dirs.replaceChildren();
        els.groups.replaceChildren(emptyState(problem ? problem.kind : 'connecting', problem && problem.text));
        lastModel = null;
        return;
      }
      const model = buildModel(data.sessions, view);
      lastModel = model;
      renderDirs(model);
      const nodes = [];
      // A stale board stays visible under a slim banner when the connection drops.
      if (problem && problem.kind !== 'connecting') nodes.push(el('div', { class: 'stale' }, emptyState(problem.kind, problem.text)));
      if (model.empty) nodes.push(emptyState(model.empty));
      model.groups.forEach((g) => {
        nodes.push(el('section', { class: 'group' },
          el('h2', { class: 'group-title' + (g.isProject ? ' is-project' : ''), title: g.path || '' },
            el('span', { text: g.path ? basename(g.path) : '未知目录' }),
            el('span', { class: 'group-path', text: g.path || '' })),
          el('div', { class: 'cards' }, g.cards.map((c) => card(c, now)))));
      });
      els.groups.replaceChildren(...nodes);
    }

    return {
      setData(reply) { data = reply; problem = null; draw(); },
      setProblem(p) { problem = p; draw(); },
      setView(next) { view = Object.assign({}, view, next); draw(); },
      redraw: draw,
      get model() { return lastModel; },
      get data() { return data; },
      find(id) { return data && Array.isArray(data.sessions) ? data.sessions.find((s) => s.session_id === id) || null : null; },
      dirs() { return lastModel ? lastModel.dirs : []; }
    };
  }

  QB.board = { buildModel, projectDir, createBoard, isActive };
})(typeof window !== 'undefined' ? window : globalThis);
