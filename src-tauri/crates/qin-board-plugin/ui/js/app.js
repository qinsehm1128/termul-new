/* qin-board · app: routing, startup, host integration (theme, context, storage
 * prefs, invalidate/resync, visibility) and the wiring between transport,
 * refreshers, board and chat. Classic script loaded last. */
(function (root) {
  'use strict';
  const QB = root.QB;
  const doc = root.document;
  const $ = (id) => doc.getElementById(id);
  const PREFS_KEY = 'qin-board.prefs';
  const BOARD_PERIOD_MS = 10000;
  const TODO_PERIOD_MS = 10000;

  const se = root.se;
  const cleanups = [];
  let disposed = false;
  let page = 'board';
  let problem = null;           // board-level problem: {kind, text}
  let lastRefresh = null;
  let projectPath = null;
  let followProject = true;     // select the project dir once data arrives
  let prefs = { dir: null, filter: 'active', todoOpen: true };
  let prefsRevision = null;
  let prefsTimer = null;
  let toastTimer = null;

  const requestId = () => 'qin-board-' + (root.crypto && root.crypto.randomUUID ? root.crypto.randomUUID() : Date.now() + '-' + Math.random().toString(16).slice(2));
  const visible = () => doc.visibilityState === 'visible';

  // ---------------- toast ----------------
  function toast(text) {
    const node = $('toast');
    node.textContent = text;
    node.hidden = false;
    root.clearTimeout(toastTimer);
    toastTimer = root.setTimeout(() => { node.hidden = true; }, 5000);
  }

  // ---------------- standalone (no host shim) ----------------
  if (!se || !se.ready) {
    doc.addEventListener('DOMContentLoaded', () => {
      $('conn').dataset.tone = 'fail';
      $('conn-text').textContent = '未在宿主中运行';
      $('refresh').disabled = true;
      $('board-groups').replaceChildren(QB.render.el('div', { class: 'empty' }, QB.render.el('p', { class: 'empty-title', text: '请从 termul 打开此视图' })));
    });
    return;
  }

  // ---------------- core objects ----------------
  const transport = QB.transport.createTransport({ invoke: (cmd, args) => se.invoke(cmd, args) });
  const api = QB.api.createApi(transport);
  let board = null;
  let chat = null;

  const boardRefresher = QB.refresh.createRefresher({
    intervalMs: BOARD_PERIOD_MS,
    isActive: () => !disposed && page === 'board' && visible() && transport.state === 'open',
    load: loadBoard,
    onChange: drawTopbar
  });
  const todoRefresher = QB.refresh.createRefresher({
    intervalMs: TODO_PERIOD_MS,
    isActive: () => !disposed && page === 'chat' && visible() && transport.state === 'open' && !!(chat && chat.session),
    load: () => chat.loadTodos().then(() => { lastRefresh = Date.now(); }),
    onChange: drawTopbar
  });
  function syncRefreshers(reason) { boardRefresher.sync(reason); todoRefresher.sync(reason); }

  async function loadBoard() {
    try {
      const reply = await api.listBoard();
      if (disposed) return;
      problem = null;
      lastRefresh = Date.now();
      board.setData({ generated_at_ms: reply.generated_at_ms, sessions: Array.isArray(reply.sessions) ? reply.sessions : [] });
      applyProjectSelection();
      if (chat.session) { const s = board.find(chat.session.session_id); if (s) chat.updateMeta(s); }
    } catch (error) {
      if (disposed) return;
      if (QB.api.isUpgradeError(error)) problem = { kind: 'upgrade', text: null };
      else if (error && error.code === 'chan_closed' && transport.state === 'open') problem = { kind: 'qin', text: QB.render.message(error) };
      else if (error && error.code === 'not_connected') problem = { kind: 'connecting', text: null };
      else problem = { kind: 'qin', text: QB.render.message(error) };
      board.setProblem(problem);
      throw error;
    } finally {
      drawTopbar();
    }
  }

  // ---------------- top bar ----------------
  function connInfo() {
    const st = transport.state;
    if (st === 'disposed') return { tone: 'fail', text: '已失效' };
    if (st === 'failed') return { tone: 'fail', text: '服务未就绪' };
    if (st === 'open') {
      if (problem && problem.kind === 'upgrade') return { tone: 'wait', text: '需要升级' };
      if (problem && problem.kind === 'qin') return { tone: 'fail', text: 'qin-code 未运行' };
      return { tone: 'run', text: '已连接' };
    }
    const d = transport.detail;
    if (st === 'backoff' && d && d.delay) return { tone: 'wait', text: '连接中 · ' + Math.round(d.delay / 1000) + ' 秒后重试' };
    return { tone: 'wait', text: '连接中' };
  }

  function drawTopbar() {
    const info = connInfo();
    $('conn').dataset.tone = info.tone;
    $('conn-text').textContent = info.text;
    $('last-refresh').textContent = lastRefresh ? '上次刷新 ' + QB.render.clock(lastRefresh) : '';
    const busy = page === 'board' ? boardRefresher.busy : todoRefresher.busy;
    const btn = $('refresh');
    btn.disabled = busy;
    btn.classList.toggle('is-busy', busy);
    btn.setAttribute('aria-busy', busy ? 'true' : 'false');
  }

  function onTransportState(state) {
    if (disposed) return;
    if (state === 'failed') {
      problem = { kind: 'down', text: transport.detail && transport.detail.error };
      board.setProblem(problem);
    } else if (state !== 'open' && state !== 'disposed' && !(problem && problem.kind === 'revoked')) {
      // Keep the last board visible; the top bar explains the connection state.
      problem = { kind: 'connecting', text: '连接中…' };
      board.setProblem(problem);
    }
    if (state !== 'open' && chat.session) chat.lost();
    syncRefreshers('transport');
    drawTopbar();
  }

  function onConnected() {
    if (disposed) return;
    // Back online. The 'open' state already crossed the refreshers' inactive→active
    // edge (board refreshes immediately); the chat page must also re-attach.
    syncRefreshers('connected');
    if (page === 'chat' && chat.session) void chat.attach();
  }

  // ---------------- routing ----------------
  function setPage(next) {
    page = next;
    $('page-board').hidden = next !== 'board';
    $('page-chat').hidden = next !== 'chat';
    $('board-tools').hidden = next !== 'board';
    doc.body.dataset.page = next;
    syncRefreshers('page');
    drawTopbar();
  }

  function openSession(session) {
    if (!session || !session.session_id) return;
    const full = board.find(session.session_id) || session;
    chat.enter(full);
    setPage('chat'); // inactive→active edge loads todos immediately
  }

  function backToBoard() {
    chat.leave();
    setPage('board'); // the inactive→active edge refreshes the board immediately
  }

  // ---------------- prefs (se.storage, profile scope, revision CAS) ----------------
  async function loadPrefs() {
    try {
      const res = await se.storage.get({ requestId: requestId(), scope: 'profile', key: PREFS_KEY });
      if (!res || !Number.isSafeInteger(res.revision)) return;
      prefsRevision = res.revision;
      const v = res.value;
      if (v && typeof v === 'object') {
        if (typeof v.dir === 'string' || v.dir === null) prefs.dir = v.dir;
        if (v.filter === 'active' || v.filter === 'all') prefs.filter = v.filter;
        if (typeof v.todoOpen === 'boolean') prefs.todoOpen = v.todoOpen;
      }
    } catch (error) {
      prefsRevision = null; // no revision → never write blindly
    }
  }

  function savePrefs() {
    root.clearTimeout(prefsTimer);
    prefsTimer = root.setTimeout(async () => {
      if (prefsRevision == null || disposed) return;
      const value = { dir: prefs.dir, filter: prefs.filter, todoOpen: prefs.todoOpen };
      try {
        const res = await se.storage.set({ requestId: requestId(), scope: 'profile', key: PREFS_KEY, value, expectedRevision: prefsRevision });
        if (res && Number.isSafeInteger(res.revision)) prefsRevision = res.revision;
      } catch (error) {
        if (error && error.code === 'PLUGIN_STORAGE_CONFLICT') {
          // Another view wrote newer prefs: adopt them instead of overwriting.
          await loadPrefs();
          applyPrefs();
        }
      }
    }, 400);
  }

  function applyPrefs() {
    $('filter-all').setAttribute('aria-pressed', prefs.filter === 'all' ? 'true' : 'false');
    $('filter-active').setAttribute('aria-pressed', prefs.filter === 'active' ? 'true' : 'false');
    $('page-chat').classList.toggle('todo-closed', !prefs.todoOpen);
    $('todo-toggle').setAttribute('aria-expanded', prefs.todoOpen ? 'true' : 'false');
    $('todo-toggle').classList.toggle('is-active', prefs.todoOpen);
    board.setView({ filter: prefs.filter, dir: prefs.dir, projectPath });
  }

  // ---------------- context: active project ----------------
  async function readProject() {
    try {
      const snap = await se.context.snapshot({ scope: 'activeProject' });
      const project = snap && snap.context ? snap.context.activeProject : null;
      const next = project && typeof project.path === 'string' ? project.path : null;
      if (next !== projectPath) { projectPath = next; followProject = true; }
      board.setView({ projectPath });
      applyProjectSelection();
    } catch (_) { /* context is optional */ }
  }

  /** Select the active project's directory once (startup / project switch) when the board knows it. */
  function applyProjectSelection() {
    if (!followProject || !projectPath || !board.data) return;
    followProject = false;
    const dir = QB.board.projectDir(board.dirs(), projectPath);
    if (dir && dir !== prefs.dir) { prefs.dir = dir; board.setView({ dir }); savePrefs(); }
  }

  // ---------------- host events ----------------
  function onInvalidate(event) {
    if (event && event.code && event.code !== 'PLUGIN_UI_INVALIDATED') {
      // Session revoked / generation changed: stop for good; the host reopens the view.
      dispose();
      problem = { kind: 'revoked', text: '视图已失效，请重新打开' };
      board.setProblem(problem);
      drawTopbar();
      return;
    }
    void rediscover('invalidate');
  }

  async function rediscover(reason) {
    await transport.refetch();
    if (disposed) return;
    void readProject();
    if (page === 'board') void boardRefresher.trigger(reason);
    else if (chat.session) { chat.reload(); void todoRefresher.trigger(reason); }
  }

  async function restartService() {
    toast('正在重启服务…');
    try {
      await se.invoke('restart-service', {});
    } catch (error) {
      toast('重启失败：' + QB.render.message(error));
    }
    problem = { kind: 'connecting', text: '连接中…' };
    if (!board.data) board.setProblem(problem);
    void transport.start();
  }

  function refreshNow() {
    if (transport.state === 'failed') { void transport.start(); return; }
    if (transport.state === 'backoff') { void transport.reconnectNow(); return; }
    if (page === 'board') void boardRefresher.trigger('manual');
    else { chat.reload(); void todoRefresher.trigger('manual'); }
  }

  function dispose() {
    if (disposed) return;
    disposed = true;
    boardRefresher.dispose();
    todoRefresher.dispose();
    if (chat) chat.leave();
    transport.dispose();
    root.clearTimeout(prefsTimer);
    cleanups.splice(0).forEach((fn) => { try { fn(); } catch (_) { /* host may be gone */ } });
  }

  // ---------------- startup ----------------
  async function start() {
    board = QB.board.createBoard(
      { dirs: $('dirs'), groups: $('board-groups') },
      {
        open: openSession,
        selectDir: (dir) => { prefs.dir = dir; followProject = false; board.setView({ dir }); savePrefs(); },
        restart: () => { void restartService(); },
        retry: () => { problem = null; refreshNow(); }
      });
    chat = QB.chat.createChat(
      {
        scroll: $('chat-scroll'), list: $('chat-list'), empty: $('chat-empty'), thinking: $('chat-thinking'),
        perms: $('perms'), banner: $('chat-banner'), title: $('chat-title'), dir: $('chat-dir'), state: $('chat-state'),
        model: $('chat-model'), sub: $('chat-sub'), parent: $('chat-parent'), input: $('composer-input'),
        send: $('composer-send'), stop: $('composer-stop'), toBottom: $('to-bottom'),
        todoList: $('todo-list'), todoCount: $('todo-count'),
        todoBadge: $('todo-toggle-count'), todoBar: $('todo-bar'), todoFill: $('todo-fill')
      },
      {
        api,
        open: openSession,
        findSession: (id) => board.find(id),
        isConnected: () => transport.state === 'open',
        notify: toast,
        turnEnded: () => { void todoRefresher.trigger('turn'); }
      });

    $('back').addEventListener('click', backToBoard);
    $('refresh').addEventListener('click', refreshNow);
    $('search').addEventListener('input', (e) => board.setView({ query: e.target.value }));
    $('search').addEventListener('keydown', (e) => { if (e.key === 'Escape') { e.target.value = ''; board.setView({ query: '' }); } });
    const setFilter = (f) => { prefs.filter = f; applyPrefs(); savePrefs(); };
    $('filter-all').addEventListener('click', () => setFilter('all'));
    $('filter-active').addEventListener('click', () => setFilter('active'));
    $('todo-toggle').addEventListener('click', () => { prefs.todoOpen = !prefs.todoOpen; applyPrefs(); savePrefs(); });
    $('todo-close').addEventListener('click', () => { prefs.todoOpen = false; applyPrefs(); savePrefs(); });
    doc.addEventListener('keydown', (e) => { if (e.key === 'Escape' && page === 'chat' && doc.activeElement !== $('composer-input')) backToBoard(); });

    transport.on('state', onTransportState);
    transport.on('connected', onConnected);
    transport.on('chanClosed', (chan, reason, info) => {
      if (info && info.local) return;
      if (chat.session && chan === api.sessionChan(chat.session.session_id) && transport.state === 'open') chat.chanClosed(reason);
    });
    const onVisibility = () => {
      syncRefreshers('visible');
      if (visible() && transport.state === 'backoff') void transport.reconnectNow();
    };
    doc.addEventListener('visibilitychange', onVisibility);
    cleanups.push(() => doc.removeEventListener('visibilitychange', onVisibility));

    setPage('board');
    board.setProblem({ kind: 'connecting', text: '连接中…' });
    drawTopbar();

    try {
      await se.ready;
    } catch (error) {
      $('conn').dataset.tone = 'fail';
      $('conn-text').textContent = '宿主连接失败';
      board.setProblem({ kind: 'standalone', text: QB.render.message(error) });
      return;
    }
    if (disposed) return;

    try {
      const theme = await se.theme.get();
      if (theme && theme.appearance) doc.documentElement.dataset.appearance = theme.appearance;
      cleanups.push(se.theme.onChange((t) => { if (t && t.appearance) doc.documentElement.dataset.appearance = t.appearance; }));
    } catch (_) { /* tokens fall back to the dark defaults in app.css */ }
    cleanups.push(se.onInvalidate(onInvalidate));
    cleanups.push(se.onResyncRequired(() => { void rediscover('resync'); }));
    // Manifest subscription 'project' ↔ topic project.activeChanged; onEvent only adds a local listener.
    cleanups.push(se.context.onEvent('project', () => { followProject = true; void readProject(); }));

    await loadPrefs();
    if (disposed) return;
    applyPrefs();
    await readProject();
    if (disposed) return;
    void transport.start();
  }

  root.addEventListener('pagehide', dispose, { once: true });
  if (doc.readyState === 'loading') doc.addEventListener('DOMContentLoaded', () => { void start(); }, { once: true });
  else void start();
})(window);
