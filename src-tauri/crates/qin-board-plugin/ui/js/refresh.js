/* qin-board · refresh: the refresh state machine.
 *
 * One refresher owns one periodic load (board: list_board; chat: get_session_todos).
 *  - Mutual exclusion: at most one load in flight. Triggers that arrive while a
 *    load is in flight coalesce into exactly one follow-up load.
 *  - Periodic timer: runs only while isActive() (page shown + document visible +
 *    connected). The countdown restarts after every completed load, so a manual
 *    or event-driven refresh pushes the next tick a full period away.
 *  - sync(): call whenever page / visibility / connection changes. Becoming
 *    active refreshes immediately; becoming inactive stops the timer.
 * Pure logic: timers and the clock are injectable so it runs under node. */
(function (root) {
  'use strict';
  const QB = (root.QB = root.QB || {});

  function createRefresher(opts) {
    const o = opts || {};
    const load = o.load;
    const intervalMs = o.intervalMs == null ? 10000 : o.intervalMs;
    const isActive = o.isActive || (() => true);
    const setT = o.setTimeout || root.setTimeout.bind(root);
    const clearT = o.clearTimeout || root.clearTimeout.bind(root);
    const now = o.now || (() => Date.now());
    const onChange = o.onChange || (() => {});

    let inFlight = null;     // Promise of the running load
    let queued = null;       // {promise, resolve} for the single coalesced follow-up
    let timer = null;
    let wasActive = false;
    let disposed = false;
    const stats = { runs: 0, coalesced: 0, lastOkAt: null, lastError: null, lastReason: null };

    function stopTimer() { if (timer != null) { clearT(timer); timer = null; } }

    function schedule() {
      stopTimer();
      if (disposed || !isActive()) return;
      timer = setT(() => {
        timer = null;
        if (!disposed && isActive()) void trigger('timer');
      }, intervalMs);
    }

    function run(reason) {
      stopTimer();
      stats.runs += 1;
      stats.lastReason = reason;
      let p = null;
      p = Promise.resolve()
        .then(() => load(reason))
        .then(
          () => { stats.lastOkAt = now(); stats.lastError = null; return { ok: true }; },
          (error) => { stats.lastError = error; return { ok: false, error }; }
        )
        .then((result) => {
          inFlight = null;
          if (disposed) return result;
          if (queued) {
            const next = queued;
            queued = null;
            run('coalesced').then(next.resolve);
          } else {
            onChange({ busy: false, stats });
            schedule();
          }
          return result;
        });
      inFlight = p;
      onChange({ busy: true, stats }); // observers read `busy` → inFlight must already be set
      return p;
    }

    /** Request a refresh now. Resolves once data covering this request is loaded. */
    function trigger(reason) {
      if (disposed) return Promise.resolve({ ok: false, error: new Error('disposed') });
      if (inFlight) {
        // Every trigger during a load merges into the single queued follow-up.
        stats.coalesced += 1;
        if (!queued) {
          let resolve;
          const promise = new Promise((r) => { resolve = r; });
          queued = { promise, resolve };
        }
        return queued.promise;
      }
      return run(reason || 'manual');
    }

    /** Re-evaluate isActive(); refresh immediately on an inactive → active edge. */
    function sync(reason) {
      if (disposed) return;
      const active = !!isActive();
      if (active && !wasActive) { wasActive = true; void trigger(reason || 'activate'); return; }
      wasActive = active;
      if (!active) stopTimer();
      else if (!inFlight && timer == null) schedule();
    }

    return {
      trigger,
      sync,
      stop() { stopTimer(); wasActive = false; },
      dispose() { disposed = true; stopTimer(); if (queued) { queued.resolve({ ok: false }); queued = null; } },
      get busy() { return !!inFlight; },
      get timerArmed() { return timer != null; },
      stats
    };
  }

  QB.refresh = { createRefresher };
})(typeof window !== 'undefined' ? window : globalThis);
