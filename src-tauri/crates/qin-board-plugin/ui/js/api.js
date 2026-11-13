/* qin-board · api: request/reply correlation over transport channels.
 * Request frame {v:1,id:N,req:...}; the reply carries reply_to:N. Frames
 * without reply_to are stream events for that channel. Classic script. */
(function (root) {
  'use strict';
  const QB = (root.QB = root.QB || {});

  const DEFAULT_TIMEOUT_MS = 20000;
  const CONTROL = 'control';
  const sessionChan = (id) => 's:' + id;

  function apiError(frame) {
    const err = new Error((frame && frame.message) || '请求失败');
    err.code = (frame && frame.code) || 'internal';
    err.api = true;
    return err;
  }

  /** True when an old qin-code rejected a request it does not know (API < 1.3). */
  function isUpgradeError(error) {
    if (!error) return false;
    if (error.code === 'unknown_request') return true;
    // Older servers may fold unknown tags into invalid_request ("unknown variant `list_board`").
    return error.code === 'invalid_request' && /unknown (variant|request)/i.test(String(error.message || ''));
  }

  function createApi(transport, opts) {
    const o = opts || {};
    const setT = o.setTimeout || root.setTimeout.bind(root);
    const clearT = o.clearTimeout || root.clearTimeout.bind(root);
    let nextId = 1;
    const pending = new Map(); // id -> {chan, resolve, reject, timer, fire}
    const eventListeners = new Map(); // chan -> Set(fn)

    transport.on('frame', (chan, frame) => {
      if (Number.isInteger(frame.reply_to) && pending.has(frame.reply_to)) {
        const p = pending.get(frame.reply_to);
        if (p.chan === chan) {
          pending.delete(frame.reply_to);
          clearT(p.timer);
          if (frame.ev === 'error') p.reject(apiError(frame));
          else p.resolve(frame);
          return;
        }
      }
      const set = eventListeners.get(chan);
      if (set) set.forEach((fn) => { try { fn(frame); } catch (e) { root.console && root.console.error(e); } });
    });

    transport.on('chanClosed', (chan, reason) => {
      for (const [id, p] of Array.from(pending.entries())) {
        if (p.chan !== chan) continue;
        pending.delete(id);
        clearT(p.timer);
        p.reject(Object.assign(new Error(reason || '通道已关闭'), { code: 'chan_closed' }));
      }
    });

    /** Send `body` and resolve with the reply frame (rejects on ev:error / timeout / close). */
    function request(chan, body, ropts) {
      const timeoutMs = (ropts && ropts.timeoutMs) || DEFAULT_TIMEOUT_MS;
      return new Promise((resolve, reject) => {
        const id = nextId++;
        const timer = setT(() => {
          if (!pending.has(id)) return;
          pending.delete(id);
          reject(Object.assign(new Error('请求超时'), { code: 'timeout' }));
        }, timeoutMs);
        pending.set(id, { chan, resolve, reject, timer });
        try {
          transport.send(chan, Object.assign({ v: 1, id }, body));
        } catch (error) {
          pending.delete(id);
          clearT(timer);
          reject(error);
        }
      });
    }

    /** Fire a request whose success has no reply (send_message streams instead).
     * An error reply, if any, is still delivered through onError. */
    function fire(chan, body, onError) {
      const p = request(chan, body, { timeoutMs: 60000 });
      // timeout: success has no reply; chan_closed: the frame already left, its fate is unknown.
      p.catch((error) => { if (error && error.code !== 'timeout' && error.code !== 'chan_closed' && onError) onError(error); });
    }

    function onEvents(chan, fn) {
      if (!eventListeners.has(chan)) eventListeners.set(chan, new Set());
      eventListeners.get(chan).add(fn);
      return () => {
        const set = eventListeners.get(chan);
        if (!set) return;
        set.delete(fn);
        if (!set.size) eventListeners.delete(chan);
      };
    }

    async function control() { await transport.openChan(CONTROL); return CONTROL; }

    return {
      request,
      fire,
      onEvents,
      // ---- control channel (stateless) ----
      async listBoard(limit) {
        const chan = await control();
        const reply = await request(chan, limit ? { req: 'list_board', limit } : { req: 'list_board' });
        if (reply.ev !== 'board') throw Object.assign(new Error('意外回复：' + reply.ev), { code: 'internal' });
        return reply;
      },
      async getTodos(sessionId) {
        const chan = await control();
        const reply = await request(chan, { req: 'get_session_todos', session_id: sessionId });
        return Array.isArray(reply.items) ? reply.items : [];
      },
      async peek(sessionId, limit) {
        const chan = await control();
        const reply = await request(chan, { req: 'peek_session', session_id: sessionId, limit: limit || 60 });
        return Array.isArray(reply.messages) ? reply.messages : [];
      },
      // ---- per-session channel ----
      sessionChan,
      async openSession(sessionId, workingDir) {
        const chan = sessionChan(sessionId);
        await transport.openChan(chan);
        const body = { req: 'attach_session', session_id: sessionId };
        if (workingDir) body.working_dir = workingDir;
        const reply = await request(chan, body, { timeoutMs: 30000 });
        return reply.session || null;
      },
      closeSession(sessionId) { transport.closeChan(sessionChan(sessionId)); },
      async history(sessionId) {
        const reply = await request(sessionChan(sessionId), { req: 'get_history', session_id: sessionId }, { timeoutMs: 30000 });
        return Array.isArray(reply.messages) ? reply.messages : [];
      },
      sendMessage(sessionId, content, onError) {
        fire(sessionChan(sessionId), { req: 'send_message', session_id: sessionId, content }, onError);
      },
      softInterrupt(sessionId, content) {
        return request(sessionChan(sessionId), { req: 'soft_interrupt', session_id: sessionId, content });
      },
      cancel(sessionId) {
        return request(sessionChan(sessionId), { req: 'cancel', session_id: sessionId });
      },
      permission(sessionId, requestId, decision) {
        return request(sessionChan(sessionId), { req: 'permission_response', session_id: sessionId, request_id: requestId, decision });
      },
      pendingCount: () => pending.size
    };
  }

  QB.api = { createApi, isUpgradeError, sessionChan, CONTROL };
})(typeof window !== 'undefined' ? window : globalThis);
