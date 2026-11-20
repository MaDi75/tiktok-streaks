const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const PRESETS = ['🔥', '❤️', '👋', '🫶', '😂', '💀', 'Привет!', '.'];
const $ = id => document.getElementById(id);

let cfg = null;
let status = {};      // имя -> {status, detail} в текущем сеансе
let busy = false;

const today = () => {
  const d = new Date();
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
};
const esc = s => String(s ?? '').replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));

function toast(text, err = false) {
  const t = $('toast');
  t.textContent = text;
  t.className = 'toast show' + (err ? ' err' : '');
  clearTimeout(toast.timer);
  toast.timer = setTimeout(() => (t.className = 'toast'), err ? 6000 : 3000);
}

function log(name, st, detail) {
  const box = $('log');
  if (box.querySelector('.muted')) box.innerHTML = '';
  const time = new Date().toLocaleTimeString('ru-RU', { hour: '2-digit', minute: '2-digit' });
  const label = { ok: '✓ отправлено', error: '✗ ошибка', skipped: '— пропущен', sending: '… отправка', info: '' }[st] ?? st;
  const div = document.createElement('div');
  div.className = 'e';
  div.innerHTML = `<span class="t">${time}</span><span class="${st}"><b>${esc(name)}</b> ${label}${detail && st !== 'sending' ? (label ? ': ' : '') + esc(detail) : ''}${st === 'error' ? ' <a href="#" class="diagLink">самопроверка</a>' : ''}</span>`;
  box.prepend(div);
}

let saveTimer;
function save() {
  clearTimeout(saveTimer);
  saveTimer = setTimeout(() => invoke('save_config', { cfg }).catch(e => toast(String(e), true)), 300);
}

function setBusy(b) {
  busy = b;
  for (const id of ['btnSendAll', 'btnRefresh', 'btnCheck', 'btnDiag']) $(id).disabled = b;
  document.querySelectorAll('.one').forEach(x => (x.disabled = b));
}

function renderPresets() {
  $('presets').innerHTML = PRESETS.map(p =>
    `<button class="preset ${cfg.defaultMessage === p ? 'on' : ''}" data-p="${esc(p)}">${esc(p)}</button>`).join('');
  $('defaultMessage').value = cfg.defaultMessage;
}

function sentStatus(name) {
  const s = status[name];
  if (s) {
    if (s.status === 'ok') return ['ok', '✓ отправлено'];
    if (s.status === 'error') return ['error', '✗ ошибка'];
    if (s.status === 'sending') return ['sending', 'отправка…'];
    if (s.status === 'skipped') return ['ok', '✓ сегодня'];
  }
  if (cfg.history[name] === today()) return ['ok', '✓ сегодня'];
  return cfg.marked.includes(name) ? ['', 'ещё нет'] : ['', ''];
}

function replyStatus(name) {
  if (!cfg.marked.includes(name)) return ['', ''];
  if (cfg.replied[name] === today()) return ['ok', '💬 ответил'];
  if (cfg.repliesChecked === today()) return ['error', '⚠ не ответил'];
  return ['', ''];
}

function renderChats() {
  const q = $('search').value.trim().toLowerCase();
  const only = $('onlyMarked').checked;
  const marked = new Set(cfg.marked);
  const all = [...cfg.chats];
  for (const m of cfg.marked) if (!all.some(c => c.name === m)) all.push({ name: m, avatar: '', last: '(нет в последнем списке)' });
  const list = all
    .filter(c => (!q || c.name.toLowerCase().includes(q)) && (!only || marked.has(c.name)))
    .sort((a, b) => marked.has(b.name) - marked.has(a.name));

  $('markedCount').textContent = cfg.marked.length;
  $('hint').innerHTML = cfg.chats.length ? '' :
    '<b>Как начать:</b><br>1. Нажми <b>«Показать браузер»</b> — откроется отдельное окно Edge. Войди там в свой аккаунт (это нужно один раз).<br>2. Нажми <b>«↻ Обновить список»</b>.<br>3. Отметь 🔥 тех, с кем у тебя серия, и выбери, что отправлять.';

  $('chats').innerHTML = list.map(c => {
    const on = marked.has(c.name);
    const [cls, txt] = sentStatus(c.name);
    const [rcls, rtxt] = replyStatus(c.name);
    const err = status[c.name]?.status === 'error' ? ` title="${esc(status[c.name].detail)}"` : '';
    const ava = c.avatar
      ? `<img class="ava" src="${esc(c.avatar)}" referrerpolicy="no-referrer" onerror="this.outerHTML='<div class=ava>${esc(c.name[0] || '?').toUpperCase()}</div>'">`
      : `<div class="ava">${esc((c.name[0] || '?').toUpperCase())}</div>`;
    return `<div class="chat ${on ? 'marked' : ''}" data-name="${esc(c.name)}">
      ${ava}
      <div class="info"><div class="name">${esc(c.name)}</div><div class="last">${esc(c.last)}</div></div>
      ${on ? `<input class="msg" placeholder="${esc(cfg.defaultMessage)}" value="${esc(cfg.perPerson[c.name] || '')}" title="Своё сообщение для этого человека">` : '<span></span>'}
      <div class="status"><span class="${cls}"${err}>${txt}</span><span class="${rcls}">${rtxt}</span></div>
      <div class="btns">
        <button class="fire ${on ? 'on' : ''}" title="${on ? 'Убрать из серий' : 'У нас серия'}">🔥</button>
        <button class="one" title="Отправить только ему сейчас" ${busy ? 'disabled' : ''}>➤</button>
      </div>
    </div>`;
  }).join('');
}

// ---------- Telegram ----------

async function renderTelegram(checkToken = false) {
  const has = !!cfg.telegramToken;
  $('tgNoToken').classList.toggle('hidden', has);
  $('tgHasToken').classList.toggle('hidden', !has);
  if (!has) return;
  const paired = cfg.telegramChatId !== 0;
  $('tgPair').classList.toggle('hidden', paired);
  $('btnTgUnlink').classList.toggle('hidden', !paired);
  if (paired) $('tgCode').classList.add('hidden');
  $('tgStatus').innerHTML = paired ? '<span class="ok">✓ Бот привязан</span>' : '<span class="sending">Бот добавлен, но ещё не привязан</span>';
  if (checkToken) {
    try {
      const u = await invoke('tg_test', { token: cfg.telegramToken });
      $('tgStatus').innerHTML += ` <span class="muted">(${esc(u)})</span>`;
    } catch (e) {
      $('tgStatus').innerHTML = `<span class="error">✗ ${esc(e)}</span>`;
    }
  }
}

$('btnTgSave').onclick = async () => {
  const token = $('tgToken').value.trim();
  if (!/^\d+:[\w-]{20,}$/.test(token)) return toast('Это не похоже на токен бота', true);
  try {
    const u = await invoke('tg_test', { token });
    cfg.telegramToken = token;
    cfg.telegramChatId = 0;
    await invoke('save_config', { cfg });
    $('tgToken').value = '';
    toast(`Бот ${u} добавлен`);
    renderTelegram(true);
  } catch (e) { toast(String(e), true); }
};
$('btnTgCode').onclick = async () => {
  const code = await invoke('tg_pair_code');
  $('tgCode').classList.remove('hidden');
  $('tgCode').innerHTML = `Отправь своему боту в Telegram:<br><b class="code">/start ${code}</b>`;
};
$('btnTgUnlink').onclick = async () => {
  await invoke('tg_unlink');
  cfg = await invoke('get_config');
  renderTelegram();
};
$('btnTgRemove').onclick = async () => {
  cfg.telegramToken = '';
  cfg.telegramChatId = 0;
  await invoke('save_config', { cfg });
  renderTelegram();
};

// ---------- действия ----------

async function reloadCfg() {
  cfg = await invoke('get_config');
  renderChats();
}

async function refresh() {
  setBusy(true);
  toast('Загружаю список чатов…');
  try {
    cfg.chats = await invoke('refresh_chats');
    toast(`Загружено чатов: ${cfg.chats.length}`);
  } catch (e) {
    toast(String(e), true);
  } finally {
    setBusy(false);
    renderChats();
  }
}

async function sendAll() {
  if (!cfg.marked.length) return toast('Сначала отметь 🔥 людей, с кем у тебя серия', true);
  setBusy(true);
  status = {};
  try {
    const s = await invoke('send_all');
    toast(`Готово: отправлено ${s.sent}, пропущено ${s.skipped}, ошибок ${s.failed}`, s.failed > 0);
  } catch (e) {
    toast(String(e), true);
  } finally {
    setBusy(false);
    reloadCfg();
  }
}

async function sendOne(name) {
  setBusy(true);
  try {
    await invoke('send_one', { name });
  } catch (e) {
    toast(String(e), true);
  } finally {
    setBusy(false);
    reloadCfg();
  }
}

async function checkReplies() {
  if (!cfg.marked.length) return toast('Сначала отметь 🔥 людей, с кем у тебя серия', true);
  setBusy(true);
  $('checkResult').textContent = 'Проверяю… это может занять пару минут.';
  try {
    const r = await invoke('check_replies');
    let t = `Ответили: ${r.replied.length}, не ответили: ${r.missing.length}`;
    if (r.unknown.length) t += `, не удалось проверить: ${r.unknown.join(', ')}`;
    $('checkResult').textContent = t;
    log('Защита серии', 'info', r.missing.length ? `не ответили — ${r.missing.join(', ')}` : 'все ответили 🔥');
  } catch (e) {
    $('checkResult').textContent = '';
    toast(String(e), true);
  } finally {
    setBusy(false);
    reloadCfg();
  }
}

async function runDiag() {
  $('diagCard').classList.remove('hidden');
  $('diagCard').scrollIntoView({ behavior: 'smooth' });
  $('diag').innerHTML = '<div class="muted">Проверяю…</div>';
  setBusy(true);
  try {
    const items = await invoke('diagnose');
    $('diag').innerHTML = items.map(i => {
      const icon = i.ok === true ? '<span class="ok">✓</span>' : i.ok === false ? '<span class="error">✗</span>' : '<span class="muted">•</span>';
      return `<div class="di">${icon}<div><b>${esc(i.title)}</b>${i.detail ? `<div class="muted">${esc(i.detail)}</div>` : ''}</div></div>`;
    }).join('');
  } catch (e) {
    $('diag').innerHTML = `<div class="error">${esc(e)}</div>`;
  } finally {
    setBusy(false);
  }
}

// ---------- события ----------

$('btnOpen').onclick = () => invoke('open_browser').catch(e => toast(String(e), true));
$('btnRefresh').onclick = refresh;
$('btnSendAll').onclick = sendAll;
$('btnCheck').onclick = checkReplies;
$('btnDiag').onclick = runDiag;
$('diagClose').onclick = () => $('diagCard').classList.add('hidden');
$('log').addEventListener('click', e => { if (e.target.classList.contains('diagLink')) { e.preventDefault(); if (!busy) runDiag(); } });
$('search').oninput = renderChats;
$('onlyMarked').onchange = renderChats;

$('presets').onclick = e => {
  const b = e.target.closest('.preset');
  if (!b) return;
  cfg.defaultMessage = b.dataset.p;
  renderPresets(); renderChats(); save();
};
$('defaultMessage').oninput = e => {
  cfg.defaultMessage = e.target.value || '🔥';
  document.querySelectorAll('.preset').forEach(p => p.classList.toggle('on', p.dataset.p === cfg.defaultMessage));
  save();
};

$('chats').addEventListener('click', e => {
  const row = e.target.closest('.chat');
  if (!row) return;
  const name = row.dataset.name;
  if (e.target.closest('.fire')) {
    cfg.marked = cfg.marked.includes(name) ? cfg.marked.filter(n => n !== name) : [...cfg.marked, name];
    save(); renderChats();
  } else if (e.target.closest('.one') && !busy) {
    sendOne(name);
  }
});
$('chats').addEventListener('input', e => {
  if (!e.target.classList.contains('msg')) return;
  const name = e.target.closest('.chat').dataset.name;
  const v = e.target.value;
  if (v.trim()) cfg.perPerson[name] = v; else delete cfg.perPerson[name];
  save();
});

$('autoEnabled').onchange = e => { cfg.autoEnabled = e.target.checked; save(); };
$('autoTime').onchange = e => { cfg.autoTime = e.target.value || '20:00'; save(); };
$('skipToday').onchange = e => { cfg.skipIfSentToday = e.target.checked; save(); };
$('guardEnabled').onchange = e => { cfg.guardEnabled = e.target.checked; save(); };
$('guardTime').onchange = e => { cfg.guardTime = e.target.value || '21:00'; save(); };
$('hideBrowser').onchange = e => {
  cfg.hideBrowser = e.target.checked; save();
  if (cfg.hideBrowser) invoke('hide_browser_now').catch(() => {});
};
$('autostart').onchange = e => invoke('set_autostart', { enabled: e.target.checked }).catch(err => toast(String(err), true));

$('btnSaveSel').onclick = () => {
  try {
    cfg.selectors = JSON.parse($('selectors').value);
    save(); toast('Селекторы сохранены');
  } catch { toast('Ошибка в JSON', true); }
};

listen('send-progress', ({ payload }) => {
  status[payload.name] = payload;
  log(payload.name, payload.status, payload.detail);
  if (payload.status === 'ok') cfg.history[payload.name] = today();
  renderChats();
});
listen('send-started', () => setBusy(true));
listen('send-finished', () => { setBusy(false); reloadCfg(); });
listen('check-started', () => setBusy(true));
listen('check-finished', () => { setBusy(false); reloadCfg(); });
listen('telegram-paired', async () => {
  cfg = await invoke('get_config');
  renderTelegram(true);
  toast('Telegram-бот привязан ✓');
});

// ---------- старт ----------

(async () => {
  cfg = await invoke('get_config');
  $('autoEnabled').checked = cfg.autoEnabled;
  $('autoTime').value = cfg.autoTime;
  $('skipToday').checked = cfg.skipIfSentToday;
  $('guardEnabled').checked = cfg.guardEnabled;
  $('guardTime').value = cfg.guardTime;
  $('hideBrowser').checked = cfg.hideBrowser;
  $('selectors').value = JSON.stringify(cfg.selectors, null, 2);
  $('autostart').checked = await invoke('get_autostart').catch(() => false);
  renderPresets();
  renderChats();
  renderTelegram(true);
})();
