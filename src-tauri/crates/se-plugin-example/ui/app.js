/* Activity Studio: static classic script. The host injects window.se; no Tauri imports. */
(function () {
  'use strict';
  const $ = (id) => document.getElementById(id);
  const se = window.se;
  const svgNS = 'http://www.w3.org/2000/svg';
  const noteKey = 'activity-studio.notes';
  const cleanups = [];
  const timeline = [];
  let connected = false;
  let disposed = false;
  let latestSummary = null;
  let servicePort = null;
  let noteRevision = null;
  let noteLoaded = false;
  let noteBusy = false;
  let savedNote = '';
  let snapshotTimer = null;
  let fetchController = null;
  const format = new Intl.NumberFormat(undefined, { maximumFractionDigits: 0 });
  const message = (error) => String(error && error.message || error || 'Unknown error').slice(0, 240);
  const text = (id, value) => { $(id).textContent = String(value); };
  const state = (id, value) => { $(id).dataset.state = value; };
  const requestId = () => 'studio-' + crypto.randomUUID();
  const metric = (value) => typeof value === 'number' && Number.isFinite(value) ? format.format(value) : '—';

  function updateNoteControls() {
    $('save-note').disabled = !connected || !noteLoaded || noteBusy || $('notes').value === savedNote;
    $('reload-note').disabled = noteBusy;
  }

  function applyTheme(theme) {
    if (!theme) return;
    // The shim owns token projection onto :root, including the host's HSL channel tokens.
    document.documentElement.style.colorScheme = theme.appearance === 'light' ? 'light' : 'dark';
    text('theme-label', 'Host theme · ' + (theme.appearance === 'light' ? 'light' : 'dark'));
  }

  function renderChart(activity) {
    const samples = Array.isArray(activity) ? activity.slice(-12) : [];
    const points = samples.map((item) => {
      const value = item.count ?? item.value ?? item.agentTurnCount ?? item.terminalCount ?? 0;
      return { label: String(item.label || 'Activity').slice(0, 24), value: typeof value === 'number' && Number.isFinite(value) ? Math.max(0, value) : 0 };
    });
    const grid = $('chart-grid');
    const series = $('chart-series');
    grid.replaceChildren();
    series.replaceChildren();
    function shape(tag, attributes, parent) {
      const element = document.createElementNS(svgNS, tag);
      Object.entries(attributes).forEach(([key, value]) => element.setAttribute(key, String(value)));
      parent.appendChild(element);
      return element;
    }
    [20, 60, 100, 140].forEach((y) => shape('line', { x1: 12, x2: 588, y1: y, y2: y, class: 'chart-gridline' }, grid));
    const max = Math.max(1, ...points.map((point) => point.value));
    const slot = 576 / Math.max(1, points.length);
    points.forEach((point, index) => {
      const height = point.value / max * 115;
      const bar = shape('rect', { x: 12 + index * slot + slot * .2, y: 140 - height, width: slot * .6, height, rx: 3, class: 'chart-bar' }, series);
      shape('title', {}, bar).textContent = point.label + ': ' + format.format(point.value);
      // Limit visible labels; every value remains in the accessible description.
      if (points.length <= 6 || index % 2 === 0) {
        shape('text', { x: 12 + (index + .5) * slot, y: 165, 'text-anchor': 'middle', class: 'chart-label' }, series).textContent = point.label.slice(0, 10);
      }
    });
    const description = points.length ? points.map((point) => point.label + ': ' + format.format(point.value)).join('; ') : 'No activity reported by the native plugin.';
    text('chart-description', description);
    text('chart-caption', points.length ? 'Latest ' + points.length + ' native activity samples · peak ' + format.format(max) : 'No activity yet. Your workspace signals will appear here.');
  }

  function renderSummary(summary) {
    if (!summary || typeof summary !== 'object') return;
    latestSummary = summary;
    text('count', metric(summary.count));
    text('terminals', metric(summary.terminalCount));
    text('agent-turns', metric(summary.agentTurnCount));
    text('wake-count', metric(summary.wakeCount));
    renderChart(summary.activity);
    const service = summary.service || {};
    servicePort = Number.isInteger(service.port) && service.port > 0 && service.port < 65536 ? service.port : null;
    text('service-port', servicePort === null ? 'Not available' : servicePort);
    text('service-process', (service.pid ?? '—') + ' / ' + (Number.isFinite(service.uptimeMs) ? Math.floor(service.uptimeMs / 1000) + 's' : '—'));
    text('service-status', service.ok === true ? 'Healthy' : service.error ? 'Unavailable' : 'Waiting');
    state('service-status', service.ok === true ? 'ok' : service.error ? 'error' : 'pending');
    text('native-ping-result', service.error ? String(service.error).slice(0, 180) : service.ok === true ? 'OK · native snapshot' : 'Awaiting native result');
    $('direct-ping').disabled = !connected || servicePort === null || !!fetchController;
  }

  async function nativeCommand(command, button) {
    button.disabled = true;
    text('native-status', command === 'service-ping' ? 'Native ping requested; waiting for a pushed service snapshot…' : 'Running ' + command + '…');
    try {
      const result = await se.invoke(command, {});
      if (disposed) return;
      renderSummary(result);
      text('native-status', command === 'service-ping' ? 'Snapshot received. The native ping runs in the background and pushes its result.' : 'Native ' + command + ' completed.');
      state('native-status', 'ok');
    } catch (error) {
      text('native-status', command + ': ' + message(error));
      state('native-status', 'error');
    } finally { if (!disposed) button.disabled = !connected; }
  }

  async function loadNote() {
    noteBusy = true;
    updateNoteControls();
    text('note-status', 'Loading latest revision…');
    try {
      const result = await se.storage.get({ requestId: requestId(), scope: 'profile', key: noteKey });
      if (disposed) return;
      if (!result || !Number.isSafeInteger(result.revision) || result.revision < 0) throw new Error('Invalid storage revision');
      const value = result.value;
      savedNote = value && typeof value.text === 'string' ? value.text : '';
      noteRevision = result.revision;
      noteLoaded = true;
      $('notes').value = savedNote;
      $('notes').disabled = false;
      $('note-conflict').hidden = true;
      text('note-revision', result.found ? 'Revision ' + noteRevision : 'New note · revision ' + noteRevision);
      text('note-status', result.found ? 'Loaded from plugin storage.' : 'Ready for your first note.');
      state('note-status', 'ok');
    } catch (error) {
      text('note-status', 'Could not load notes: ' + message(error));
      state('note-status', 'error');
      $('note-conflict').hidden = false;
      $('note-conflict').querySelector('p').textContent = 'Storage is unavailable. Retry loading before you edit.';
    } finally { noteBusy = false; updateNoteControls(); }
  }

  async function saveNote() {
    if (!noteLoaded || noteBusy) return;
    const draft = $('notes').value;
    noteBusy = true;
    updateNoteControls();
    text('note-status', 'Saving with revision check…');
    try {
      const result = await se.storage.set({ requestId: requestId(), scope: 'profile', key: noteKey, value: { text: draft }, expectedRevision: noteRevision });
      if (disposed) return;
      if (!result || !Number.isSafeInteger(result.revision)) throw new Error('Invalid storage revision');
      noteRevision = result.revision;
      savedNote = draft;
      text('note-revision', 'Revision ' + noteRevision);
      text('note-status', $('notes').value === draft ? 'Saved. A little clarity, kept.' : 'Saved previous draft. New edits are unsaved.');
      state('note-status', 'ok');
    } catch (error) {
      if (error && error.code === 'PLUGIN_STORAGE_CONFLICT') {
        noteLoaded = false; // Never retry with a newer revision and silently overwrite another author.
        $('note-conflict').hidden = false;
        $('note-conflict').querySelector('p').textContent = 'Another view saved newer notes. Your draft is still here.';
        text('note-status', 'Revision conflict. Copy your draft, then explicitly load latest.');
      } else text('note-status', 'Save failed; your draft is preserved: ' + message(error));
      state('note-status', 'error');
    } finally { noteBusy = false; updateNoteControls(); }
  }

  function addTimeline(title, detail) {
    timeline.unshift({ title, detail, at: new Date() });
    timeline.splice(12);
    $('timeline').replaceChildren(...timeline.map((entry) => {
      const item = document.createElement('li');
      const heading = document.createElement('strong');
      heading.textContent = entry.title;
      const time = document.createElement('time');
      time.dateTime = entry.at.toISOString();
      time.textContent = entry.at.toLocaleTimeString();
      const body = document.createElement('p');
      body.textContent = entry.detail;
      item.append(heading, time, body);
      return item;
    }));
  }

  async function refreshContext() {
    try {
      const snapshot = await se.context.snapshot({ scope: 'activeProject' });
      if (disposed) return;
      const context = snapshot.context || {};
      const project = context.activeProject;
      text('project-label', project ? project.name || project.id || 'Active workspace' : 'No active project');
      addTimeline('Context snapshot', (context.terminals || []).length + ' terminals · ' + (context.agentSessions || []).length + ' agent sessions · revision ' + (snapshot.contextRevision ?? '—'));
    } catch (error) { addTimeline('Context unavailable', message(error)); }
  }

  function onContext(event) {
    if (disposed) return;
    const data = event.data || {};
    let detail = event.projectId ? 'Project ' + event.projectId : 'Workspace context updated';
    if (data.terminal) detail = 'Terminal ' + (data.terminal.title || data.terminal.id || 'updated');
    if (data.session) detail = 'Agent ' + data.session.id + ' · ' + data.session.status;
    addTimeline(event.topic || 'Context event', detail);
    // Resync and project switches need a fresh authoritative snapshot, not a guessed local reducer.
    if (event.resyncRequired || event.topic === 'context.resync' || event.topic === 'project.activeChanged') {
      refreshAuthoritative();
    }
  }

  function subscribeContext() {
    // Subscriptions are declared in the manifest; onEvent only registers a local listener.
    const subscriptions = ['project', 'terminal-created', 'terminal-closed', 'terminal-active', 'activity'];
    subscriptions.forEach((subscriptionId) => {
      cleanups.push(se.context.onEvent(subscriptionId, onContext));
    });
    text('context-status', subscriptions.length + ' listeners');
  }

  function refreshAuthoritative() {
    clearTimeout(snapshotTimer);
    snapshotTimer = setTimeout(() => {
      snapshotTimer = null;
      if (!disposed) {
        void refreshContext();
        void nativeCommand('summary', $('summary'));
      }
    }, 100);
  }

  async function readConfig() {
    try {
      const config = await se.config.get();
      const values = config.values || {};
      text('global-status', 'Connected · counter step ' + (values.step ?? 1) + ' · ' + (values.greeting || 'Native signals update this view live.'));
    } catch (error) { text('global-status', 'Configuration unavailable: ' + message(error)); }
  }

  async function directPing() {
    if (servicePort === null || fetchController) return;
    const controller = new AbortController();
    fetchController = controller;
    $('direct-ping').disabled = true;
    text('direct-ping-result', 'Fetching from sandbox…');
    const timeout = setTimeout(() => controller.abort(), 5000);
    try {
      // Fixed loopback host, validated integer port, no credentials or redirects.
      const response = await fetch('http://127.0.0.1:' + servicePort + '/ping', { mode: 'cors', credentials: 'omit', cache: 'no-store', redirect: 'error', signal: controller.signal });
      if (!response.ok) throw new Error('HTTP ' + response.status);
      const reply = await response.json();
      if (!reply || reply.ok !== true) throw new Error('Invalid service response');
      text('direct-ping-result', 'HTTP ' + response.status + ' · CORS allowed · PID ' + metric(reply.pid) + ' · ' + metric(reply.uptimeMs) + ' ms');
      state('direct-ping-result', 'ok');
    } catch (error) {
      text('direct-ping-result', error.name === 'AbortError' ? 'Timed out / cancelled' : 'Blocked or unreachable: ' + message(error));
      state('direct-ping-result', 'error');
    } finally {
      clearTimeout(timeout);
      fetchController = null;
      if (!disposed) $('direct-ping').disabled = !connected || servicePort === null;
    }
  }

  function checkSandbox() {
    const observations = [];
    ['__TAURI__', '__TAURI_INTERNALS__', '__TAURI_IPC__'].forEach((key) => {
      try { observations.push([key, 'typeof = ' + typeof window[key] + (typeof window[key] === 'undefined' ? ' · Not exposed' : ' · EXPOSED — inspect host boundary'), typeof window[key] !== 'undefined']); }
      catch (_) { observations.push([key, 'Access denied', false]); }
    });
    try {
      if (window.parent === window) observations.push(['Parent document', 'Top-level preview; isolation not tested', true]);
      else {
        void window.parent.document.documentElement;
        observations.push(['Parent document', 'READABLE — inspect host boundary', true]);
      }
    } catch (_) { observations.push(['Parent document', 'Access denied by browser', false]); }
    observations.push(['document.origin', String(document.origin) + ' (non-standard; may be undefined)', false]);
    observations.push(['Window security origin', String(window.origin), window.origin !== 'null']);
    observations.push(['location.origin', location.origin + ' (URL, not security origin)', false]);
    $('sandbox-results').replaceChildren(...observations.map(([label, detail, warning]) => {
      const item = document.createElement('li');
      const heading = document.createElement('strong');
      heading.textContent = label;
      item.append(heading, document.createTextNode(detail));
      if (warning) item.dataset.state = 'error';
      return item;
    }));
  }

  async function notify() {
    $('notify').disabled = true;
    try {
      await se.feedback.notify({ requestId: requestId(), id: 'activity-studio-summary', severity: 'info', title: 'Activity Studio', message: 'Counter: ' + metric(latestSummary && latestSummary.count) });
      text('native-status', 'Notification delivered through the host API.');
      state('native-status', 'ok');
    } catch (error) { text('native-status', 'Notification: ' + message(error)); state('native-status', 'error'); }
    finally { if (!disposed) $('notify').disabled = !connected; }
  }

  async function start() {
    checkSandbox();
    renderChart([]);
    $('check-sandbox').addEventListener('click', checkSandbox);
    if (!se || !se.ready) {
      text('bridge-status', 'Host bridge unavailable');
      state('bridge-status', 'error');
      text('global-status', 'Open this UI from Termul. A standalone preview cannot access native commands or persisted state.');
      text('note-status', 'Host storage is unavailable in this preview.');
      return;
    }
    ['increment', 'reset', 'summary', 'service-ping'].forEach((command) => {
      $(command).addEventListener('click', () => { void nativeCommand(command, $(command)); });
    });
    $('notify').addEventListener('click', () => { void notify(); });
    $('direct-ping').addEventListener('click', () => { void directPing(); });
    $('save-note').addEventListener('click', () => { void saveNote(); });
    $('reload-note').addEventListener('click', () => { void loadNote(); });
    $('notes').addEventListener('input', () => { text('note-status', 'Unsaved draft'); updateNoteControls(); });
    try {
      await se.ready;
      if (disposed) return;
      connected = true;
      applyTheme(await se.theme.get());
      cleanups.push(se.theme.onChange(applyTheme));
      cleanups.push(se.onMessage(renderSummary));
      cleanups.push(se.onResyncRequired(refreshAuthoritative));
      cleanups.push(se.onInvalidate((event) => {
        if (event && event.code && event.code !== 'PLUGIN_UI_INVALIDATED') {
          connected = false;
          ['increment', 'reset', 'summary', 'service-ping', 'direct-ping', 'notify', 'save-note'].forEach((id) => { $(id).disabled = true; });
          text('bridge-status', 'Session revoked');
          state('bridge-status', 'error');
        } else refreshAuthoritative();
      }));
      cleanups.push(se.config.onChange(() => { void readConfig(); }));
      text('bridge-status', 'Host connected');
      state('bridge-status', 'ok');
      text('global-status', 'Connected through the versioned plugin bridge. Native signals update this view live.');
      ['increment', 'reset', 'summary', 'service-ping', 'notify'].forEach((id) => { $(id).disabled = false; });
      await Promise.allSettled([nativeCommand('summary', $('summary')), refreshContext(), subscribeContext(), loadNote(), readConfig()]);
    } catch (error) {
      text('bridge-status', 'Connection failed');
      state('bridge-status', 'error');
      text('global-status', message(error));
    }
  }

  window.addEventListener('pagehide', () => {
    disposed = true;
    connected = false;
    clearTimeout(snapshotTimer);
    if (fetchController) fetchController.abort();
    cleanups.splice(0).forEach((cleanup) => { try { cleanup(); } catch (_) { /* Host may already have detached. */ } });
  }, { once: true });
  void start();
})();
