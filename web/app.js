'use strict';
/* Writer-Chat — frontend. Sem dependências externas. */

const $ = (s, r = document) => r.querySelector(s);
const el = (tag, cls, text) => { const e = document.createElement(tag); if (cls) e.className = cls; if (text != null) e.textContent = text; return e; };
const uid = () => (crypto.randomUUID ? crypto.randomUUID() : Date.now().toString(36) + Math.random().toString(36).slice(2, 10));
const countWords = s => (String(s).match(/\S+/g) || []).length;
const esc = s => String(s).replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));

const SAMPLING_DEFAULTS = { temperature: 0.9, top_p: 0.95, top_k: 40, min_p: 0.05, repeat_penalty: 1.05 };
const DEFAULTS = { model: '', system: '', words: 400, thinking: false, ...SAMPLING_DEFAULTS };
const IDEAS = [
  'Escreva a abertura de um conto de ficção científica noir passado em uma cidade chuvosa.',
  'Crie uma cena de diálogo tensa entre dois irmãos que não se falam há dez anos.',
  'Desenvolva a sinopse e os três atos de um romance de fantasia sombria.',
  'Escreva um poema em versos livres sobre o último dia de verão.',
];

const state = {
  cfg: null,
  settings: { ...DEFAULTS },
  convs: [],
  cur: null,
  streaming: false,
  ctrl: null,
  pending: [], // anexos do rascunho atual
  online: false,
};

/* ---------- Persistência de preferências ---------- */
function loadSettings() {
  try { Object.assign(state.settings, JSON.parse(localStorage.getItem('writerchat.settings') || '{}')); } catch { /* ignora */ }
}
function saveSettings() {
  try { localStorage.setItem('writerchat.settings', JSON.stringify(state.settings)); } catch { /* ignora */ }
}

/* ---------- API ---------- */
async function api(path, opts) {
  const r = await fetch(path, opts);
  if (!r.ok) {
    let msg = r.statusText;
    try { msg = (await r.json()).error || msg; } catch { /* ignora */ }
    throw new Error(msg);
  }
  return r.status === 204 ? null : r.json();
}

function toast(msg, kind = '', ms = 5000) {
  const t = el('div', 'toast ' + kind, msg);
  $('#toasts').appendChild(t);
  setTimeout(() => t.remove(), ms);
}

/* ---------- Markdown mínimo e seguro (escapa HTML antes) ---------- */
function inline(s) {
  s = esc(s);
  s = s.replace(/`([^`\n]+)`/g, '<code>$1</code>');
  s = s.replace(/\*\*([^*\n]+?)\*\*/g, '<strong>$1</strong>');
  s = s.replace(/__([^_\n]+?)__/g, '<strong>$1</strong>');
  s = s.replace(/(^|[^*\w])\*([^*\s][^*\n]*?)\*(?!\*)/g, '$1<em>$2</em>');
  s = s.replace(/(^|[^\w])_([^_\s][^_\n]*?)_(?!\w)/g, '$1<em>$2</em>');
  return s;
}
function md(src) {
  const out = [];
  let para = [], list = null, quote = [], code = null;
  const flushPara = () => { if (para.length) { out.push('<p>' + para.map(inline).join('<br>') + '</p>'); para = []; } };
  const flushList = () => { if (list) { out.push(`<${list.t}>` + list.items.map(i => '<li>' + inline(i) + '</li>').join('') + `</${list.t}>`); list = null; } };
  const flushQuote = () => { if (quote.length) { out.push('<blockquote>' + quote.map(inline).join('<br>') + '</blockquote>'); quote = []; } };
  const flush = () => { flushPara(); flushList(); flushQuote(); };
  for (const line of src.replace(/\r/g, '').split('\n')) {
    if (code) {
      if (/^```/.test(line)) { out.push('<pre><code>' + esc(code.join('\n')) + '</code></pre>'); code = null; } else code.push(line);
      continue;
    }
    if (/^```/.test(line)) { flush(); code = []; continue; }
    if (!line.trim()) { flush(); continue; }
    let m;
    if ((m = line.match(/^(#{1,4})\s+(.*)$/))) { flush(); const n = m[1].length + 1; out.push(`<h${n}>${inline(m[2])}</h${n}>`); continue; }
    if (/^\s*([-*_])(\s*\1){2,}\s*$/.test(line)) { flush(); out.push('<hr>'); continue; }
    if ((m = line.match(/^>\s?(.*)$/))) { flushPara(); flushList(); quote.push(m[1]); continue; }
    if ((m = line.match(/^\s*[-*+]\s+(.*)$/))) { flushPara(); flushQuote(); if (!list || list.t !== 'ul') { flushList(); list = { t: 'ul', items: [] }; } list.items.push(m[1]); continue; }
    if ((m = line.match(/^\s*\d+[.)]\s+(.*)$/))) { flushPara(); flushQuote(); if (!list || list.t !== 'ol') { flushList(); list = { t: 'ol', items: [] }; } list.items.push(m[1]); continue; }
    flushList(); flushQuote();
    para.push(line);
  }
  if (code) out.push('<pre><code>' + esc(code.join('\n')) + '</code></pre>');
  flush();
  return out.join('');
}

/* ---------- Raciocínio (<think>) ---------- */
/** Separa raciocínio e resposta a partir do texto bruto em streaming. */
function parseOutput(m, expectThink, done) {
  const raw = m._raw || '';
  let reasoning = m._rf || '';
  let content = raw;
  const close = raw.indexOf('</think>');
  if (close >= 0) {
    reasoning += raw.slice(0, close).replace(/^\s*<think>/, '');
    content = raw.slice(close + 8);
  } else if (/^\s*<think>/.test(raw)) {
    reasoning += raw.replace(/^\s*<think>/, '');
    content = '';
  } else if (expectThink && !m._rf && !done) {
    reasoning = raw; content = '';
  }
  m.reasoning = reasoning.trim();
  m.content = content.replace(/^\s+/, '');
}

/* ---------- Conversas ---------- */
function newConv() {
  return { id: uid(), title: '', created_at: Date.now(), updated_at: Date.now(), messages: [] };
}
function stripTransient(_k, v) { return _k.startsWith('_') ? undefined : v; }

let saveTimer = null;
function scheduleSave(now = false) {
  clearTimeout(saveTimer);
  const run = async () => {
    const c = state.cur;
    if (!c || !c.messages.length) return;
    c.updated_at = Date.now();
    try {
      await api('/api/conversations/' + encodeURIComponent(c.id), {
        method: 'PUT', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(c, stripTransient),
      });
      upsertSummary(c);
      try { localStorage.setItem('writerchat.last', c.id); } catch { /* ignora */ }
      renderConvList();
    } catch (e) { toast('Não consegui salvar a conversa: ' + e.message, 'error'); }
  };
  if (now) run(); else saveTimer = setTimeout(run, 400);
}
function upsertSummary(c) {
  const s = { id: c.id, title: c.title || 'Nova história', updated_at: c.updated_at, messages: c.messages.length };
  const i = state.convs.findIndex(x => x.id === c.id);
  if (i >= 0) state.convs[i] = s; else state.convs.unshift(s);
  state.convs.sort((a, b) => b.updated_at - a.updated_at);
}

async function openConv(id) {
  if (state.streaming) return toast('Aguarde a resposta terminar (ou clique em Parar).');
  try {
    const c = await api('/api/conversations/' + encodeURIComponent(id));
    c.messages = c.messages || [];
    state.cur = c;
    localStorage.setItem('writerchat.last', id);
  } catch (e) { toast('Erro ao abrir: ' + e.message, 'error'); return; }
  state.pending = [];
  renderAll(); closeSidebar();
}
function startNew() {
  if (state.streaming) return toast('Aguarde a resposta terminar (ou clique em Parar).');
  state.cur = newConv(); state.pending = [];
  try { localStorage.removeItem('writerchat.last'); } catch { /* ignora */ }
  renderAll(); closeSidebar(); $('#input').focus();
}
async function deleteConv(id) {
  if (!confirm('Apagar esta conversa? Isso não pode ser desfeito.')) return;
  try { await api('/api/conversations/' + encodeURIComponent(id), { method: 'DELETE' }); } catch (e) { return toast(e.message, 'error'); }
  state.convs = state.convs.filter(c => c.id !== id);
  if (state.cur && state.cur.id === id) { state.streaming ? state.ctrl?.abort() : null; state.cur = newConv(); }
  renderAll();
}

/* ---------- Render ---------- */
function renderConvList() {
  const box = $('#conv-list'); box.textContent = '';
  if (!state.convs.length) { box.appendChild(el('div', 'empty-list', 'Suas histórias aparecerão aqui.')); return; }
  for (const c of state.convs) {
    const row = el('div', 'conv' + (state.cur && c.id === state.cur.id ? ' active' : ''));
    row.tabIndex = 0;
    const t = el('div', 'ct'); t.appendChild(el('div', '', c.title || 'Nova história'));
    t.appendChild(el('div', 'cm', `${c.messages} msg · ${new Date(c.updated_at).toLocaleDateString('pt-BR')}`));
    const del = el('button', 'del', '✕'); del.title = 'Apagar'; del.setAttribute('aria-label', 'Apagar conversa');
    del.onclick = e => { e.stopPropagation(); deleteConv(c.id); };
    row.append(t, del);
    row.onclick = () => openConv(c.id);
    row.onkeydown = e => { if (e.key === 'Enter') openConv(c.id); };
    box.appendChild(row);
  }
}

function attachChip(a, onRemove) {
  const chip = el('div', 'att' + (a.error ? ' err' : '') + (a.loading ? ' loading' : ''));
  if (a.kind === 'image' && a.data_url) { const i = el('img'); i.src = a.data_url; i.alt = a.name; chip.appendChild(i); }
  else chip.appendChild(el('span', 'ico', a.error ? '⚠' : '📄'));
  const info = el('div');
  const n = el('span', 'an', a.name); n.title = a.name;
  let sub = a.loading ? 'enviando…' : a.error ? a.error : a.kind === 'image' ? 'imagem' : `${(a.words || 0).toLocaleString('pt-BR')} palavras${a.truncated ? ' (truncado)' : ''}`;
  info.append(n, el('span', 'as', sub));
  chip.appendChild(info);
  if (onRemove) { const b = el('button', 'rm', '✕'); b.title = 'Remover'; b.setAttribute('aria-label', 'Remover anexo'); b.onclick = onRemove; chip.appendChild(b); }
  return chip;
}

function lengthTag(m) {
  if (!m.words || m.role !== 'assistant' || !m.content) return null;
  const n = countWords(m.content);
  const ratio = n / m.words;
  const cls = ratio >= 0.8 && ratio <= 1.25 ? 'ok' : 'warn';
  const t = el('span', 'tag ' + cls, `${n.toLocaleString('pt-BR')} / ~${m.words.toLocaleString('pt-BR')} palavras`);
  t.title = 'Palavras geradas / alvo de tamanho';
  return t;
}

function buildMessage(m, idx, last) {
  const wrap = el('div', `msg ${m.role}${last ? ' last' : ''}`); wrap.dataset.id = m.id;
  const who = el('div', 'who', m.role === 'user' ? 'Você' : 'Writer');
  wrap.appendChild(who);

  if (m.role === 'user' && m.attachments?.length) {
    const al = el('div', 'attach-list'); m.attachments.forEach(a => al.appendChild(attachChip(a))); wrap.appendChild(al);
  }
  const bubble = el('div', 'bubble');
  if (m.role === 'user') bubble.textContent = m.content;
  wrap.appendChild(bubble);
  fillAssistant(wrap, m);

  const meta = el('div', 'meta');
  if (m.role === 'assistant') {
    const lt = lengthTag(m); if (lt) meta.appendChild(lt);
    if (m.finish === 'length') meta.appendChild(Object.assign(el('span', 'tag warn', 'cortada pelo limite'), { title: 'Atingiu o limite de tokens. Clique em Continuar.' }));
    if (m.finish === 'stopped') meta.appendChild(el('span', 'tag', 'interrompida'));
    if (m.error) meta.appendChild(el('span', 'tag warn', 'erro'));
  }
  const acts = el('div', 'actions');
  const act = (label, title, fn) => { const b = el('button', 'act', label); b.title = title; b.onclick = fn; acts.appendChild(b); };
  if (!state.streaming || !last) {
    if (m.role === 'assistant') {
      act('Copiar', 'Copiar texto', () => copyText(m.content));
      if (last) { act('↻ Regenerar', 'Gerar outra versão', () => regenerate()); act('Continuar', 'Continuar de onde parou', () => continueStory()); }
    } else {
      act('Editar', 'Editar e reenviar', () => startEdit(m, wrap));
    }
    act('Apagar', 'Apagar daqui para baixo', () => deleteFrom(idx));
  }
  meta.appendChild(acts);
  wrap.appendChild(meta);
  return wrap;
}

/** Preenche o corpo (raciocínio + texto) de uma mensagem do assistente. */
function fillAssistant(wrap, m) {
  if (m.role !== 'assistant') return;
  const bubble = $('.bubble', wrap);
  const live = state.streaming && m._live;
  bubble.textContent = '';
  if (m.reasoning) {
    const d = el('details', 'think');
    if (live && !m.content) d.open = true;
    d.appendChild(el('summary', '', `Raciocínio · ${countWords(m.reasoning)} palavras`));
    d.appendChild(el('div', 'think-body', m.reasoning));
    bubble.appendChild(d);
  }
  const body = el('div'); body.innerHTML = md(m.content || ''); bubble.appendChild(body);
  bubble.classList.toggle('cursor', !!live);
  if (live && !m.content && !m.reasoning) bubble.appendChild(el('span', '', ' '));
}

function renderMessages() {
  const box = $('#messages'); box.textContent = '';
  const c = state.cur;
  if (!c || !c.messages.length) {
    const w = el('div', 'welcome');
    w.appendChild(el('h1', '', 'Sobre o que vamos escrever?'));
    w.appendChild(el('p', '', 'Converse em vários turnos: peça uma cena, ajuste o tom, mande continuar.'));
    w.appendChild(el('p', '', 'Escolha o tamanho da resposta em palavras abaixo e anexe rascunhos, pesquisas ou imagens de referência.'));
    const ideas = el('div', 'ideas');
    for (const t of IDEAS) { const b = el('button', 'idea', t); b.onclick = () => { $('#input').value = t; autosize(); $('#input').focus(); }; ideas.appendChild(b); }
    w.appendChild(ideas); box.appendChild(w); return;
  }
  const col = el('div', 'col');
  c.messages.forEach((m, i) => col.appendChild(buildMessage(m, i, i === c.messages.length - 1)));
  box.appendChild(col);
  scrollBottom(true);
}
function scrollBottom(force) {
  const b = $('#messages');
  if (force || b.scrollHeight - b.scrollTop - b.clientHeight < 140) b.scrollTop = b.scrollHeight;
}

function renderAttachDraft() {
  const box = $('#attach-list'); box.textContent = '';
  state.pending.forEach((a, i) => box.appendChild(attachChip(a, () => { state.pending.splice(i, 1); renderAttachDraft(); })));
}

function renderTop() {
  $('#title').value = state.cur?.title || '';
  const s = state.settings;
  const chip = $('#model-chip');
  const name = (s.model || state.cfg?.model || '').split('/').pop();
  chip.textContent = `${name} · ${state.cfg?.quant || ''}`; chip.title = s.model || state.cfg?.model || '';
}

function renderLength() {
  const w = state.settings.words || 0;
  const pills = $('#len-pills'); pills.textContent = '';
  for (const p of state.cfg.length_presets) {
    const b = el('button', 'pill' + (p.words === w ? ' on' : ''), p.label);
    if (p.words) b.appendChild(el('small', '', p.words));
    b.type = 'button'; b.setAttribute('aria-pressed', p.words === w);
    b.onclick = () => { state.settings.words = p.words; saveSettings(); renderLength(); };
    pills.appendChild(b);
  }
  const inp = $('#len-input');
  if (document.activeElement !== inp) inp.value = w || '';
  $('#think-toggle').checked = !!state.settings.thinking;
}

function renderComposer() {
  $('#send-btn').hidden = state.streaming;
  $('#stop-btn').hidden = !state.streaming;
  $('#attach-btn').disabled = state.streaming;
}

function renderAll() { renderConvList(); renderTop(); renderMessages(); renderAttachDraft(); renderComposer(); }

/* ---------- Envio e streaming ---------- */
function autosize() {
  const t = $('#input'); t.style.height = 'auto'; t.style.height = Math.min(t.scrollHeight, 220) + 'px';
}

async function sendMessage() {
  if (state.streaming) return;
  const text = $('#input').value.trim();
  if (state.pending.some(a => a.loading)) return toast('Aguarde o envio dos anexos.');
  const atts = state.pending.filter(a => !a.error);
  if (!text && !atts.length) return;
  const c = state.cur;
  c.messages.push({ id: uid(), role: 'user', content: text || 'Use o material anexado.', attachments: atts.map(stripAtt), ts: Date.now() });
  if (!c.title) c.title = (text || atts[0].name).replace(/\s+/g, ' ').slice(0, 48);
  $('#input').value = ''; autosize(); state.pending = [];
  renderAttachDraft();
  await generate();
}
const stripAtt = a => ({ name: a.name, kind: a.kind, text: a.text, data_url: a.data_url, words: a.words, chars: a.chars, truncated: a.truncated });

async function generate() {
  const c = state.cur, s = state.settings;
  const asst = { id: uid(), role: 'assistant', content: '', reasoning: '', words: s.words || 0, model: s.model || state.cfg.model, ts: Date.now(), finish: null, _raw: '', _rf: '', _live: true };
  const history = c.messages.filter(m => m.content?.trim() || m.attachments?.length)
    .map(m => ({ role: m.role, content: m.content, attachments: (m.attachments || []).map(a => ({ name: a.name, kind: a.kind, text: a.text, data_url: a.data_url })) }));
  c.messages.push(asst);
  state.streaming = true; state.ctrl = new AbortController();
  renderTop(); renderMessages(); renderComposer(); renderConvList();
  const wrap = $(`.msg[data-id="${asst.id}"]`);
  const expectThink = !!s.thinking;

  let raf = 0;
  const paint = () => { raf = 0; parseOutput(asst, expectThink, false); fillAssistant(wrap, asst); scrollBottom(); };
  const body = { model: s.model || undefined, system: s.system || undefined, messages: history, words: s.words || 0, thinking: expectThink,
    temperature: s.temperature, top_p: s.top_p, top_k: s.top_k || undefined, min_p: s.min_p || undefined, repeat_penalty: s.repeat_penalty > 1 ? s.repeat_penalty : undefined };

  try {
    const r = await fetch('/api/chat', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body), signal: state.ctrl.signal });
    if (!r.ok) throw new Error(await r.text() || r.statusText);
    const reader = r.body.getReader(); const dec = new TextDecoder(); let buf = '';
    for (;;) {
      const { value, done } = await reader.read();
      if (done) break;
      buf += dec.decode(value, { stream: true });
      let i;
      while ((i = buf.indexOf('\n\n')) >= 0) {
        const block = buf.slice(0, i); buf = buf.slice(i + 2);
        for (const line of block.split('\n')) {
          if (!line.startsWith('data:')) continue;
          let ev; try { ev = JSON.parse(line.slice(5)); } catch { continue; }
          if (ev.type === 'content') asst._raw += ev.text;
          else if (ev.type === 'reasoning') asst._rf += ev.text;
          else if (ev.type === 'error') { asst.error = ev.message; }
          else if (ev.type === 'done') { asst.finish = ev.finish_reason || 'stop'; asst.usage = ev.usage || undefined; }
        }
        if (!raf) raf = requestAnimationFrame(paint);
      }
    }
  } catch (e) {
    if (e.name === 'AbortError') asst.finish = 'stopped';
    else asst.error = asst.error || ('Falha na conexão: ' + e.message);
  }
  if (raf) cancelAnimationFrame(raf);
  parseOutput(asst, expectThink, true);
  if (asst.error) toast(asst.error, 'error', 9000);
  // Resposta vazia (erro antes de gerar algo): remove o balão.
  if (!asst.content && !asst.reasoning) {
    c.messages = c.messages.filter(m => m !== asst);
  } else {
    delete asst._live; asst.reasoning = asst.reasoning || '';
  }
  state.streaming = false; state.ctrl = null;
  renderAll(); scheduleSave(true);
}

function stopStreaming() { state.ctrl?.abort(); }

function regenerate() {
  if (state.streaming) return;
  const c = state.cur;
  while (c.messages.length && c.messages[c.messages.length - 1].role === 'assistant') c.messages.pop();
  if (!c.messages.length) return;
  generate();
}
function continueStory() {
  if (state.streaming) return;
  const last = state.cur.messages[state.cur.messages.length - 1];
  const cut = last?.finish === 'length';
  state.cur.messages.push({ id: uid(), role: 'user', ts: Date.now(), attachments: [],
    content: cut ? 'Sua resposta foi cortada. Continue exatamente de onde parou, sem repetir nada.' : 'Continue a história a partir de onde parou, mantendo o mesmo estilo, voz e ritmo.' });
  generate();
}
function deleteFrom(idx) {
  if (state.streaming) return;
  const n = state.cur.messages.length - idx;
  if (n > 1 && !confirm(`Apagar esta mensagem e as ${n - 1} seguintes?`)) return;
  state.cur.messages.splice(idx);
  renderAll();
  if (state.cur.messages.length) scheduleSave(true);
  else api('/api/conversations/' + encodeURIComponent(state.cur.id), { method: 'DELETE' }).then(() => { state.convs = state.convs.filter(c => c.id !== state.cur.id); renderConvList(); }).catch(() => {});
}
function startEdit(m, wrap) {
  const bubble = $('.bubble', wrap); const meta = $('.meta', wrap);
  const box = el('div', 'edit-box'); const ta = el('textarea'); ta.value = m.content;
  const row = el('div', 'row'); const ok = el('button', 'btn primary', 'Salvar e regenerar'); const no = el('button', 'btn', 'Cancelar');
  row.append(ok, no); box.append(ta, row);
  bubble.replaceWith(box); meta.hidden = true; ta.focus();
  no.onclick = () => renderMessages();
  ok.onclick = () => {
    const idx = state.cur.messages.indexOf(m);
    m.content = ta.value.trim() || m.content;
    state.cur.messages.splice(idx + 1);
    generate();
  };
}
async function copyText(t) {
  try { await navigator.clipboard.writeText(t); toast('Copiado!', '', 1500); }
  catch { toast('Não foi possível copiar automaticamente.', 'error'); }
}

/* ---------- Anexos ---------- */
async function addFiles(fileList) {
  const files = [...fileList]; if (!files.length) return;
  if (state.streaming) return toast('Espere a resposta terminar para anexar.');
  const placeholders = files.map(f => ({ name: f.name, kind: 'text', loading: true }));
  state.pending.push(...placeholders); renderAttachDraft();
  const fd = new FormData(); files.forEach(f => fd.append('file', f, f.name));
  try {
    const res = await api('/api/upload', { method: 'POST', body: fd });
    for (const p of placeholders) state.pending.splice(state.pending.indexOf(p), 1);
    for (const a of res) {
      if (a.error) toast(`${a.name}: ${a.error}`, 'error'); // não mantém anexos com erro
      else { state.pending.push(a); if (a.truncated) toast(`${a.name}: texto truncado em ${(state.cfg.max_attach_chars).toLocaleString('pt-BR')} caracteres.`); }
    }
  } catch (e) {
    for (const p of placeholders) state.pending.splice(state.pending.indexOf(p), 1);
    toast('Falha no upload: ' + e.message, 'error');
  }
  renderAttachDraft();
}

/* ---------- Exportar ---------- */
function exportMd() {
  const c = state.cur; if (!c?.messages.length) return toast('Nada para exportar ainda.');
  const parts = [`# ${c.title || 'Sem título'}\n`];
  for (const m of c.messages) {
    if (m.role === 'user') parts.push(`> **Você:** ${(m.content || '').replace(/\n/g, '\n> ')}\n`);
    else parts.push(m.content + '\n');
  }
  const blob = new Blob([parts.join('\n')], { type: 'text/markdown;charset=utf-8' });
  const a = el('a'); a.href = URL.createObjectURL(blob);
  a.download = (c.title || 'historia').replace(/[^\p{L}\p{N}]+/gu, '-').replace(/^-|-$/g, '').toLowerCase() + '.md';
  a.click(); setTimeout(() => URL.revokeObjectURL(a.href), 1000);
}

/* ---------- Configurações ---------- */
const SLIDERS = ['temperature', 'top_p', 'top_k', 'min_p', 'repeat_penalty'];
function syncSettingsUi() {
  const s = state.settings;
  $('#set-model').value = s.model; $('#set-model').placeholder = state.cfg.model;
  $('#set-system').value = s.system; $('#set-system').placeholder = state.cfg.default_system;
  for (const k of SLIDERS) { $('#set-' + k).value = s[k]; $('#v-' + k).textContent = k === 'top_k' ? (s[k] || 'off') : Number(s[k]).toFixed(2).replace(/\.?0+$/, ''); }
  $('#model-hint').textContent = `Padrão: ${state.cfg.model} (${state.cfg.quant}). Servidor: ${state.cfg.backend_url}`;
  $('#data-note').textContent = 'As conversas ficam salvas no servidor (pasta data/conversations). Preferências ficam neste navegador.';
}
function openSettings(open) {
  $('#settings').classList.toggle('open', open); $('#settings').setAttribute('aria-hidden', !open);
  if (open) syncSettingsUi();
}
function openSidebar() { $('#app').classList.add('side-open'); }
function closeSidebar() { $('#app').classList.remove('side-open'); }

async function refreshStatus() {
  try {
    const r = await api('/api/models');
    state.online = r.online;
    $('#status-dot').className = 'dot ' + (r.online ? 'on' : 'off');
    $('#status-text').textContent = r.online ? 'Modelo online' : 'Servidor do modelo offline';
    $('#status-text').title = r.error || '';
    const dl = $('#model-options'); dl.textContent = '';
    for (const id of [state.cfg.model, ...r.models.filter(m => m !== state.cfg.model)]) dl.appendChild(Object.assign(el('option'), { value: id }));
  } catch { $('#status-dot').className = 'dot off'; $('#status-text').textContent = 'Backend inacessível'; }
}

/* ---------- Eventos ---------- */
function bind() {
  $('#new-chat').onclick = startNew;
  $('#send-btn').onclick = sendMessage;
  $('#stop-btn').onclick = stopStreaming;
  $('#menu-btn').onclick = openSidebar; $('#scrim').onclick = closeSidebar;
  $('#settings-btn').onclick = () => openSettings(true); $('#settings-close').onclick = () => openSettings(false);
  $('#export-btn').onclick = exportMd;
  $('#attach-btn').onclick = () => $('#file-input').click();
  $('#file-input').onchange = e => { addFiles(e.target.files); e.target.value = ''; };
  const inp = $('#input');
  inp.addEventListener('input', autosize);
  inp.addEventListener('keydown', e => {
    if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) { e.preventDefault(); sendMessage(); }
    if (e.key === 'Escape' && state.streaming) stopStreaming();
  });
  inp.addEventListener('paste', e => { const f = [...(e.clipboardData?.files || [])]; if (f.length) { e.preventDefault(); addFiles(f); } });
  document.addEventListener('keydown', e => { if (e.key === 'Escape') { openSettings(false); closeSidebar(); } });

  // Drag & drop
  let depth = 0;
  window.addEventListener('dragenter', e => { if (e.dataTransfer?.types?.includes('Files')) { depth++; document.body.classList.add('dragging'); } });
  window.addEventListener('dragleave', () => { if (--depth <= 0) { depth = 0; document.body.classList.remove('dragging'); } });
  window.addEventListener('dragover', e => e.preventDefault());
  window.addEventListener('drop', e => { e.preventDefault(); depth = 0; document.body.classList.remove('dragging'); if (e.dataTransfer?.files?.length) addFiles(e.dataTransfer.files); });

  // Tamanho em palavras
  $('#len-input').addEventListener('input', e => {
    const v = parseInt(e.target.value, 10);
    state.settings.words = Number.isFinite(v) && v > 0 ? Math.min(8000, v) : 0;
    saveSettings();
    const keep = e.target.value; renderLength(); e.target.value = keep;
  });
  $('#len-input').addEventListener('blur', () => { const w = state.settings.words; if (w && w < 20) { state.settings.words = 20; saveSettings(); } renderLength(); });
  $('#think-toggle').onchange = e => { state.settings.thinking = e.target.checked; saveSettings(); };

  // Título
  $('#title').addEventListener('change', e => { if (!state.cur) return; state.cur.title = e.target.value.trim(); if (state.cur.messages.length) scheduleSave(true); });
  $('#title').addEventListener('keydown', e => { if (e.key === 'Enter') e.target.blur(); });

  // Configurações
  $('#set-model').addEventListener('input', e => { state.settings.model = e.target.value.trim(); saveSettings(); renderTop(); });
  $('#set-system').addEventListener('input', e => { state.settings.system = e.target.value; saveSettings(); });
  $('#reset-system').onclick = () => { state.settings.system = ''; saveSettings(); syncSettingsUi(); };
  for (const k of SLIDERS) $('#set-' + k).addEventListener('input', e => { state.settings[k] = parseFloat(e.target.value); saveSettings(); syncSettingsUi(); });
  $('#reset-sampling').onclick = () => { Object.assign(state.settings, SAMPLING_DEFAULTS); saveSettings(); syncSettingsUi(); };
}

async function init() {
  loadSettings();
  try { state.cfg = await api('/api/config'); } catch (e) { document.body.textContent = 'Backend inacessível: ' + e.message; return; }
  bind();
  try { state.convs = await api('/api/conversations'); } catch { state.convs = []; }
  state.cur = newConv();
  const last = (() => { try { return localStorage.getItem('writerchat.last'); } catch { return null; } })();
  renderLength(); renderAll();
  if (last && state.convs.some(c => c.id === last)) await openConv(last);
  refreshStatus(); setInterval(refreshStatus, 15000);
}
init();
