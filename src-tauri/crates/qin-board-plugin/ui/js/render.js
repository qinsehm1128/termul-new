/* qin-board · render: DOM helpers, formatting, and a small markdown renderer.
 * Classic script. Never uses innerHTML: every node is built with the DOM API and
 * every piece of text goes through textContent / text nodes. The parsing half is
 * pure (no document access) so it can be exercised outside a browser. */
(function (root) {
  'use strict';
  const QB = (root.QB = root.QB || {});

  // ---------- DOM helpers ----------
  const BOOL_PROPS = new Set(['hidden', 'disabled', 'open', 'checked', 'readOnly']);
  const VALUE_PROPS = new Set(['value', 'type', 'tabIndex', 'id']);

  function append(node, child) {
    if (child == null || child === false || child === true) return;
    if (Array.isArray(child)) { child.forEach((c) => append(node, c)); return; }
    node.appendChild(typeof child === 'object' ? child : root.document.createTextNode(String(child)));
  }

  /** el('div', {class, text, title, dataset, on:{click}, ...attrs}, ...children) */
  function el(tag, props, ...children) {
    const node = root.document.createElement(tag);
    if (props) {
      for (const key of Object.keys(props)) {
        const value = props[key];
        if (value == null || value === false) continue;
        if (key === 'class') node.className = value;
        else if (key === 'text') node.textContent = String(value);
        else if (key === 'dataset') Object.assign(node.dataset, value);
        else if (key === 'on') Object.keys(value).forEach((evt) => node.addEventListener(evt, value[evt]));
        else if (key === 'style') Object.assign(node.style, value);
        else if (BOOL_PROPS.has(key)) node[key] = !!value;
        else if (VALUE_PROPS.has(key)) node[key] = value;
        else node.setAttribute(key, String(value));
      }
    }
    append(node, children);
    return node;
  }

  /** A status dot. tone: run | wait | idle | done | fail */
  function dot(tone, label) {
    return el('span', { class: 'dot', dataset: { tone: tone || 'idle' }, 'aria-hidden': label ? null : 'true', title: label || null });
  }

  // ---------- formatting ----------
  const pad = (n) => String(n).padStart(2, '0');
  function clock(ms) {
    if (!Number.isFinite(ms)) return '—';
    const d = new Date(ms);
    return pad(d.getHours()) + ':' + pad(d.getMinutes()) + ':' + pad(d.getSeconds());
  }
  function relTime(ms, now) {
    if (!Number.isFinite(ms) || ms <= 0) return '';
    const sec = Math.max(0, Math.round(((now == null ? Date.now() : now) - ms) / 1000));
    if (sec < 45) return '刚刚';
    if (sec < 3600) return Math.max(1, Math.round(sec / 60)) + ' 分钟前';
    if (sec < 86400) return Math.floor(sec / 3600) + ' 小时前';
    if (sec < 86400 * 7) return Math.floor(sec / 86400) + ' 天前';
    const d = new Date(ms);
    return (d.getMonth() + 1) + '月' + d.getDate() + '日';
  }
  function normPath(p) {
    if (typeof p !== 'string' || !p) return '';
    const trimmed = p.replace(/[\\/]+$/, '');
    return trimmed || p.slice(0, 1);
  }
  function basename(p) {
    const n = normPath(p);
    if (!n) return '未知目录';
    const parts = n.split(/[\\/]/);
    return parts[parts.length - 1] || n;
  }
  function shortId(id) {
    const s = String(id || '');
    return s.length > 10 ? '…' + s.slice(-8) : s;
  }
  function truncate(s, n) {
    const str = String(s == null ? '' : s);
    return str.length > n ? str.slice(0, n) + '…' : str;
  }
  function message(error) {
    return truncate(String((error && error.message) || error || '未知错误'), 240);
  }

  const STATES = {
    streaming: { label: '生成中', tone: 'run' },
    waiting: { label: '等待中', tone: 'wait' },
    idle: { label: '空闲', tone: 'idle' },
    completed: { label: '已完成', tone: 'done' },
    failed: { label: '失败', tone: 'fail' },
    stopped: { label: '已停止', tone: 'idle' }
  };
  function stateInfo(state) {
    return STATES[state] || { label: state ? String(state) : '空闲', tone: 'idle' };
  }

  // ---------- markdown: pure parse ----------
  const FENCE_OPEN = /^\s{0,3}(`{3,}|~{3,})\s*([^\s`]*)/;
  const HEADING = /^\s{0,3}(#{1,6})\s+(.*?)\s*#*\s*$/;
  const HR = /^\s{0,3}([-*_])(\s*\1){2,}\s*$/;
  const LIST_ITEM = /^(\s*)([-*+]|\d{1,9}[.)])\s+(.*)$/;
  const QUOTE = /^\s{0,3}>\s?(.*)$/;

  /** Split markdown into blocks: p | h | code | list | quote | hr. An unclosed
   * fence (common mid-stream) swallows the rest as code. */
  function parseBlocks(src) {
    const lines = String(src == null ? '' : src).replace(/\r\n?/g, '\n').split('\n');
    const blocks = [];
    let para = [];
    const flush = () => {
      if (para.length) blocks.push({ type: 'p', text: para.join('\n') });
      para = [];
    };
    let i = 0;
    while (i < lines.length) {
      const line = lines[i];
      const fence = FENCE_OPEN.exec(line);
      if (fence) {
        flush();
        const marker = fence[1];
        const close = new RegExp('^\\s{0,3}' + (marker[0] === '`' ? '`' : '~') + '{' + marker.length + ',}\\s*$');
        const body = [];
        let closed = false;
        i++;
        while (i < lines.length) {
          if (close.test(lines[i])) { closed = true; i++; break; }
          body.push(lines[i]);
          i++;
        }
        blocks.push({ type: 'code', lang: fence[2] || '', text: body.join('\n'), closed });
        continue;
      }
      if (!line.trim()) { flush(); i++; continue; }
      const heading = HEADING.exec(line);
      if (heading) { flush(); blocks.push({ type: 'h', level: heading[1].length, text: heading[2] }); i++; continue; }
      if (HR.test(line)) { flush(); blocks.push({ type: 'hr' }); i++; continue; }
      const first = LIST_ITEM.exec(line);
      if (first && (para.length === 0 || first[1].length === 0)) {
        flush();
        const ordered = /\d/.test(first[2]);
        const items = [];
        while (i < lines.length) {
          const m = LIST_ITEM.exec(lines[i]);
          if (m && (/\d/.test(m[2]) === ordered || m[1].length > 0)) {
            items.push({ text: m[3], depth: Math.min(3, Math.floor(m[1].length / 2)) });
            i++;
          } else if (items.length && lines[i].trim() && /^\s{2,}\S/.test(lines[i])) {
            items[items.length - 1].text += '\n' + lines[i].trim();
            i++;
          } else break;
        }
        blocks.push({ type: 'list', ordered, start: ordered ? parseInt(first[2], 10) || 1 : 1, items });
        continue;
      }
      const quote = QUOTE.exec(line);
      if (quote) {
        flush();
        const body = [];
        while (i < lines.length) {
          const q = QUOTE.exec(lines[i]);
          if (!q) break;
          body.push(q[1]);
          i++;
        }
        blocks.push({ type: 'quote', text: body.join('\n') });
        continue;
      }
      para.push(line);
      i++;
    }
    flush();
    return blocks;
  }

  const ESCAPABLE = /[\\`*_[\]()#+\-.!>~|{}]/;
  const BARE_URL = /^https?:\/\/[^\s<>"'`]+/;

  /** Inline tokens: text | code | strong | em | link | br */
  function parseInline(text, depth) {
    const src = String(text == null ? '' : text);
    const level = depth || 0;
    const out = [];
    let buf = '';
    const push = () => { if (buf) { out.push({ t: 'text', v: buf }); buf = ''; } };
    let i = 0;
    while (i < src.length) {
      const c = src[i];
      if (c === '\\' && i + 1 < src.length && ESCAPABLE.test(src[i + 1])) { buf += src[i + 1]; i += 2; continue; }
      if (c === '\n') { push(); out.push({ t: 'br' }); i++; continue; }
      if (c === '`') {
        let n = 1;
        while (src[i + n] === '`') n++;
        const fence = '`'.repeat(n);
        const end = src.indexOf(fence, i + n);
        if (end > -1) {
          push();
          out.push({ t: 'code', v: src.slice(i + n, end).replace(/^ (.*) $/, '$1') });
          i = end + n;
        } else { buf += fence; i += n; }
        continue;
      }
      if (level < 4 && c === '*' && src[i + 1] === '*') {
        const end = src.indexOf('**', i + 2);
        if (end > i + 2) {
          push();
          out.push({ t: 'strong', children: parseInline(src.slice(i + 2, end), level + 1) });
          i = end + 2;
          continue;
        }
      }
      if (level < 4 && c === '*' && src[i + 1] && src[i + 1] !== ' ' && src[i + 1] !== '*') {
        let end = i + 1;
        while ((end = src.indexOf('*', end)) > -1 && (src[end + 1] === '*' || src[end - 1] === ' ')) end += 2;
        if (end > i + 1 && src.slice(i + 1, end).indexOf('\n') === -1) {
          push();
          out.push({ t: 'em', children: parseInline(src.slice(i + 1, end), level + 1) });
          i = end + 1;
          continue;
        }
      }
      if (c === '[') {
        const close = src.indexOf('](', i + 1);
        const end = close > -1 ? src.indexOf(')', close + 2) : -1;
        if (close > i && end > close && src.slice(i + 1, close).indexOf('\n') === -1) {
          push();
          const label = src.slice(i + 1, close);
          const href = src.slice(close + 2, end).trim().split(/\s+/)[0] || '';
          out.push({ t: 'link', href, children: level < 4 ? parseInline(label, level + 1) : [{ t: 'text', v: label }] });
          i = end + 1;
          continue;
        }
      }
      if ((c === 'h' || c === 'H') && (i === 0 || /[\s(（:：]/.test(src[i - 1]))) {
        const m = BARE_URL.exec(src.slice(i));
        if (m) {
          const url = m[0].replace(/[.,;:!?)）。，；：！？]+$/, '');
          push();
          out.push({ t: 'link', href: url, children: [{ t: 'text', v: url }] });
          i += url.length;
          continue;
        }
      }
      buf += c;
      i++;
    }
    push();
    return out;
  }

  // ---------- markdown: DOM ----------
  function inlineNodes(tokens) {
    return tokens.map((tok) => {
      switch (tok.t) {
        case 'text': return root.document.createTextNode(tok.v);
        case 'br': return el('br');
        case 'code': return el('code', { class: 'md-icode', text: tok.v });
        case 'strong': return el('strong', null, inlineNodes(tok.children));
        case 'em': return el('em', null, inlineNodes(tok.children));
        // Links are shown, never followed: the view is sandboxed and must not navigate.
        case 'link': return el('span', { class: 'md-link', title: tok.href || null }, inlineNodes(tok.children));
        default: return null;
      }
    });
  }

  function blockNode(block) {
    switch (block.type) {
      case 'p': return el('p', null, inlineNodes(parseInline(block.text)));
      case 'h': return el('p', { class: 'md-h md-h' + block.level }, inlineNodes(parseInline(block.text)));
      case 'hr': return el('hr');
      case 'quote': return el('blockquote', null, inlineNodes(parseInline(block.text)));
      case 'code':
        return el('pre', { class: 'md-code', dataset: block.lang ? { lang: block.lang } : null }, el('code', { text: block.text }));
      case 'list': {
        const list = el(block.ordered ? 'ol' : 'ul', { start: block.ordered && block.start !== 1 ? block.start : null });
        block.items.forEach((item) => {
          list.appendChild(el('li', { class: item.depth ? 'md-d' + item.depth : null }, inlineNodes(parseInline(item.text))));
        });
        return list;
      }
      default: return null;
    }
  }

  /** Place the streaming caret inside the deepest trailing text container. */
  function appendCursor(container) {
    let target = container;
    for (;;) {
      const last = target.lastElementChild;
      if (!last || !/^(P|LI|UL|OL|PRE|CODE|BLOCKQUOTE)$/.test(last.tagName)) break;
      target = last;
    }
    target.appendChild(el('span', { class: 'caret', 'aria-hidden': 'true' }));
  }

  function renderMarkdown(src, opts) {
    const box = el('div', { class: 'md' });
    parseBlocks(src).forEach((block) => append(box, blockNode(block)));
    if (opts && opts.cursor) appendCursor(box);
    return box;
  }

  QB.render = {
    el, append, dot, clock, relTime, normPath, basename, shortId, truncate, message,
    stateInfo, parseBlocks, parseInline, renderMarkdown
  };
})(typeof window !== 'undefined' ? window : globalThis);
