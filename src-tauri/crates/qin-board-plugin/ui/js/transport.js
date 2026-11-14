/* qin-board · transport: endpoint discovery, the sidecar WebSocket, channels,
 * heartbeat and back-off reconnect. Classic script; exports QB.transport. */
(function (root) {
  'use strict';
  const QB = (root.QB = root.QB || {});

  const ENDPOINT_TRIES = 15;
  const ENDPOINT_DELAY_MS = 1000;
  const BACKOFF_CAP_MS = 10000;
  const READY_TIMEOUT_MS = 8000;
  const CHAN_OPEN_TIMEOUT_MS = 12000;
  const PING_EVERY_MS = 15000;
  const SILENCE_LIMIT_MS = 40000;

  /** Reconnect delay for the n-th consecutive failure: 1s, 2s, 4s, 8s, then 10s. */
  function backoffDelay(attempt) {
    const n = Math.max(0, attempt | 0);
    return Math.min(1000 * Math.pow(2, n), BACKOFF_CAP_MS);
  }

  function validEndpoint(res) {
    return !!res && res.ready === true && Number.isInteger(res.port) && res.port > 0 && res.port < 65536 &&
      typeof res.token === 'string' && res.token.length > 0 && res.token.length <= 512;
  }

  /** Ask the native side for the sidecar endpoint; retry 1s apart while it is not ready.
   * Throws the last reason after `tries` attempts or when cancelled. */
  async function fetchEndpoint(invoke, opts) {
    const o = opts || {};
    const tries = o.tries || ENDPOINT_TRIES;
    const delayMs = o.delayMs == null ? ENDPOINT_DELAY_MS : o.delayMs;
    const sleep = o.sleep || ((ms) => new Promise((resolve) => root.setTimeout(resolve, ms)));
    const cancelled = o.isCancelled || (() => false);
    let last = '服务未就绪';
    for (let i = 0; i < tries; i++) {
      if (cancelled()) throw Object.assign(new Error('cancelled'), { cancelled: true });
      try {
        const res = await invoke('endpoint', {});
        if (validEndpoint(res)) return { port: res.port, token: res.token };
        last = (res && res.error) ? String(res.error) : '服务未就绪';
      } catch (error) {
        last = (error && error.message) || String(error);
      }
      if (i < tries - 1) await sleep(delayMs);
    }
    if (cancelled()) throw Object.assign(new Error('cancelled'), { cancelled: true });
    throw Object.assign(new Error(last), { code: 'endpoint_unavailable' });
  }

  /**
   * States: idle → endpoint → connecting → open; on loss → backoff → connecting …;
   * endpoint exhaustion → failed (needs restart / retry); disposed is terminal.
   */
  function createTransport(deps) {
    const d = deps || {};
    const invoke = d.invoke;
    const WS = d.WebSocket || root.WebSocket;
    const setT = d.setTimeout || root.setTimeout.bind(root);
    const clearT = d.clearTimeout || root.clearTimeout.bind(root);
    const listeners = { state: new Set(), frame: new Set(), chanClosed: new Set(), connected: new Set() };

    let state = 'idle';
    let detail = null;
    let gen = 0;
    let ws = null;
    let endpoint = null;
    let attempt = 0;
    let everConnected = false;
    let backoffTimer = null;
    let readyTimer = null;
    let pingTimer = null;
    let lastSeen = 0;
    const chans = new Map(); // chan -> {open:boolean, hello, waiters:[{resolve,reject}], timer}
    const slashWaiters = new Map();
    let slashSeq = 0;

    const emit = (kind, ...args) => listeners[kind].forEach((fn) => { try { fn(...args); } catch (e) { root.console && root.console.error(e); } });
    function setState(next, info) {
      state = next;
      detail = info || null;
      emit('state', state, detail);
    }
    function on(kind, fn) { listeners[kind].add(fn); return () => listeners[kind].delete(fn); }

    function clearTimers() {
      clearT(backoffTimer); backoffTimer = null;
      clearT(readyTimer); readyTimer = null;
      clearT(pingTimer); pingTimer = null;
    }

    function dropChans(reason) {
      const sList = Array.from(slashWaiters.values());
      slashWaiters.clear();
      sList.forEach((w) => { clearT(w.timer); w.resolve({ skills: [], commands: [] }); });
      const list = Array.from(chans.entries());
      chans.clear();
      list.forEach(([chan, c]) => {
        clearT(c.timer);
        c.waiters.forEach((w) => w.reject(Object.assign(new Error(reason), { code: 'chan_closed' })));
        emit('chanClosed', chan, reason);
      });
    }

    function teardownSocket(reason) {
      clearTimers();
      if (ws) {
        const old = ws;
        ws = null;
        old.onopen = old.onmessage = old.onclose = old.onerror = null;
        try { old.close(); } catch (_) { /* already closed */ }
      }
      dropChans(reason || '连接已断开');
    }

    function rawSend(obj) {
      if (!ws || ws.readyState !== 1) return false;
      ws.send(JSON.stringify(obj));
      return true;
    }

    async function connect(opts) {
      const my = ++gen;
      const fresh = !!(opts && opts.refetch) || !endpoint;
      // State first: listeners must see "not open" before channels are dropped.
      setState(fresh ? 'endpoint' : 'connecting');
      teardownSocket('重新连接');
      try {
        endpoint = await fetchEndpoint(invoke, {
          tries: fresh ? ENDPOINT_TRIES : 1,
          isCancelled: () => my !== gen,
          sleep: (ms) => new Promise((resolve) => setT(resolve, ms))
        });
      } catch (error) {
        if (my !== gen) return;
        if (fresh) { setState('failed', { error: error.message }); return; }
        scheduleReconnect(my, error.message); // reconnect path: keep backing off
        return;
      }
      if (my !== gen) return;
      openSocket(my);
    }

    function openSocket(my) {
      setState('connecting', { attempt });
      let sock;
      try {
        sock = new WS('ws://127.0.0.1:' + endpoint.port + '/?token=' + encodeURIComponent(endpoint.token));
      } catch (error) {
        scheduleReconnect(my, (error && error.message) || 'WebSocket 创建失败');
        return;
      }
      ws = sock;
      readyTimer = setT(() => { if (my === gen && state !== 'open') { try { sock.close(); } catch (_) { /* noop */ } } }, READY_TIMEOUT_MS);
      sock.onmessage = (event) => {
        if (my !== gen) return;
        lastSeen = Date.now();
        let msg;
        try { msg = JSON.parse(event.data); } catch (_) { return; }
        if (msg && typeof msg === 'object') handleMessage(msg, my);
      };
      sock.onclose = () => {
        if (my !== gen || ws !== sock) return;
        ws = null;
        clearTimers();
        scheduleReconnect(my, '连接已断开'); // state leaves "open" before channels are dropped
        dropChans('连接已断开');
      };
      sock.onerror = () => { /* onclose follows */ };
    }

    function scheduleReconnect(my, reason) {
      if (my !== gen || state === 'disposed') return;
      const delay = backoffDelay(attempt);
      attempt += 1;
      setState('backoff', { delay, attempt, error: reason });
      backoffTimer = setT(() => { if (my === gen) void connect({ refetch: false }); }, delay);
    }

    function handleMessage(msg, my) {
      switch (msg.op) {
        case 'ready': {
          clearT(readyTimer); readyTimer = null;
          attempt = 0;
          const reconnect = everConnected;
          everConnected = true;
          setState('open', { server: msg.server || null });
          startHeartbeat(my);
          emit('connected', { reconnect });
          break;
        }
        case 'opened': {
          const c = chans.get(msg.chan);
          if (!c) { rawSend({ op: 'close', chan: msg.chan }); break; }
          clearT(c.timer);
          c.open = true;
          c.hello = msg.hello || null;
          c.waiters.splice(0).forEach((w) => w.resolve(c.hello));
          break;
        }
        case 'frame':
          if (chans.has(msg.chan) && msg.frame && typeof msg.frame === 'object') emit('frame', msg.chan, msg.frame);
          break;
        case 'closed': {
          const c = chans.get(msg.chan);
          if (!c) break;
          chans.delete(msg.chan);
          clearT(c.timer);
          const reason = msg.error ? String(msg.error) : '通道已关闭';
          c.waiters.splice(0).forEach((w) => w.reject(Object.assign(new Error(reason), { code: 'chan_closed' })));
          emit('chanClosed', msg.chan, reason);
          break;
        }
        case 'slash_items': {
          const w = slashWaiters.get(msg.id);
          if (w) {
            slashWaiters.delete(msg.id);
            clearT(w.timer);
            w.resolve({
              skills: Array.isArray(msg.skills) ? msg.skills : [],
              commands: Array.isArray(msg.commands) ? msg.commands : []
            });
          }
          break;
        }
        case 'pong':
        default:
          break;
      }
    }

    function startHeartbeat(my) {
      clearT(pingTimer);
      const beat = () => {
        if (my !== gen || state !== 'open') return;
        if (Date.now() - lastSeen > SILENCE_LIMIT_MS && ws) { try { ws.close(); } catch (_) { /* noop */ } return; }
        rawSend({ op: 'ping' });
        pingTimer = setT(beat, PING_EVERY_MS);
      };
      pingTimer = setT(beat, PING_EVERY_MS);
    }

    /** Resolve with the channel's hello once the sidecar reports it opened. */
    function openChan(chan) {
      const existing = chans.get(chan);
      if (existing && existing.open) return Promise.resolve(existing.hello);
      return new Promise((resolve, reject) => {
        if (state !== 'open' || !ws) { reject(Object.assign(new Error('未连接'), { code: 'not_connected' })); return; }
        let c = existing;
        if (!c) {
          c = { open: false, hello: null, waiters: [], timer: null };
          chans.set(chan, c);
          c.timer = setT(() => {
            if (chans.get(chan) !== c || c.open) return;
            chans.delete(chan);
            rawSend({ op: 'close', chan });
            c.waiters.splice(0).forEach((w) => w.reject(Object.assign(new Error('打开通道超时'), { code: 'timeout' })));
          }, CHAN_OPEN_TIMEOUT_MS);
          rawSend({ op: 'open', chan });
        }
        c.waiters.push({ resolve, reject });
      });
    }

    function closeChan(chan) {
      const c = chans.get(chan);
      if (!c) return;
      chans.delete(chan);
      clearT(c.timer);
      c.waiters.splice(0).forEach((w) => w.reject(Object.assign(new Error('通道已关闭'), { code: 'chan_closed' })));
      rawSend({ op: 'close', chan });
      emit('chanClosed', chan, '通道已关闭', { local: true });
    }

    function send(chan, frame) {
      const c = chans.get(chan);
      if (!c || !c.open) throw Object.assign(new Error('通道未打开'), { code: 'chan_closed' });
      if (!rawSend({ op: 'send', chan, frame })) throw Object.assign(new Error('未连接'), { code: 'not_connected' });
    }

    function listSlash(workingDir) {
      if (state !== 'open' || !ws) return Promise.resolve({ skills: [], commands: [] });
      const id = ++slashSeq;
      return new Promise((resolve) => {
        const timer = setT(() => {
          slashWaiters.delete(id);
          resolve({ skills: [], commands: [] });
        }, 2500);
        slashWaiters.set(id, { resolve, timer });
        if (!rawSend({ op: 'list_slash', id, working_dir: workingDir || null })) {
          slashWaiters.delete(id);
          clearT(timer);
          resolve({ skills: [], commands: [] });
        }
      });
    }

    return {
      on,
      openChan,
      closeChan,
      send,
      listSlash,
      isChanOpen: (chan) => !!(chans.get(chan) && chans.get(chan).open),
      get state() { return state; },
      get detail() { return detail; },
      /** First connection, or a hard restart after the service was restarted. */
      start() { attempt = 0; return connect({ refetch: true }); },
      /** Skip any pending back-off and try now. */
      reconnectNow() { attempt = 0; return connect({ refetch: state === 'failed' }); },
      /** Re-read the endpoint (host invalidate/resync); reconnect only when it changed or we are down. */
      async refetch() {
        const my = gen;
        let next;
        try { next = await fetchEndpoint(invoke, { tries: 1 }); } catch (_) { next = null; }
        if (my !== gen || state === 'disposed') return;
        const changed = next && (!endpoint || next.port !== endpoint.port || next.token !== endpoint.token);
        if (changed) { endpoint = next; attempt = 0; await connect({ refetch: false }); }
        else if (state !== 'open' && state !== 'connecting' && state !== 'endpoint') { attempt = 0; await connect({ refetch: !next }); }
      },
      dispose() {
        gen++;
        setState('disposed');
        teardownSocket('已关闭');
      }
    };
  }

  QB.transport = { createTransport, fetchEndpoint, backoffDelay, validEndpoint };
})(typeof window !== 'undefined' ? window : globalThis);
