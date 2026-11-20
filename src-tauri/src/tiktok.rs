//! Работа со страницей tiktok.com/messages.

use crate::browser::Cdp;
use crate::store::{Chat, Selectors};
use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::time::sleep;

pub const MESSAGES_URL: &str = "https://www.tiktok.com/messages";
pub const NOT_LOGGED_IN: &str =
    "Ты не вошёл в TikTok. Окно браузера показано — войди в аккаунт и повтори.";

/// Общие JS-помощники + тело функции.
fn js(sel: &Selectors, body: &str) -> String {
    let s = serde_json::to_string(sel).unwrap();
    format!(
        r#"(async () => {{
const S = {s};
const wait = ms => new Promise(r => setTimeout(r, ms));
const items = () => [...document.querySelectorAll(S.chatItem)];
const nameOf = el => {{
  const n = S.chatName ? el.querySelector(S.chatName) : null;
  return ((n ? n.textContent : (el.innerText || '').split('\n')[0]) || '').trim();
}};
const scrollBox = () => {{
  const f = items()[0];
  let b = f ? f.parentElement : null;
  while (b && b !== document.body && b.scrollHeight <= b.clientHeight + 5) b = b.parentElement;
  return (b && b !== document.body) ? b : null;
}};
const findChat = async want => {{
  let el = items().find(e => nameOf(e) === want);
  if (el) return el;
  const box = scrollBox();
  if (!box) return null;
  box.scrollTop = 0; await wait(400);
  for (let i = 0; i < 40; i++) {{
    el = items().find(e => nameOf(e) === want);
    if (el) return el;
    const prev = box.scrollTop;
    box.scrollTop += box.clientHeight * 0.8;
    await wait(600);
    if (box.scrollTop === prev) break;
  }}
  return items().find(e => nameOf(e) === want) || null;
}};
const editor = () => {{
  const b = document.querySelector(S.input) || document.querySelector('[contenteditable="true"]');
  if (!b) return null;
  return b.matches('[contenteditable="true"]') ? b : (b.querySelector('[contenteditable="true"]') || b);
}};
const center = el => {{ const r = el.getBoundingClientRect(); return {{ x: r.left + r.width / 2, y: r.top + r.height / 2 }}; }};
const isLoggedOut = () => location.href.includes('/login') || !!(S.loginButton && document.querySelector(S.loginButton));
{body}
}})()"#
    )
}

fn point(v: &Value) -> Option<(f64, f64)> {
    Some((v.get("x")?.as_f64()?, v.get("y")?.as_f64()?))
}

/// Открывает страницу сообщений и ждёт, пока появится список чатов.
pub async fn ensure_messages_page(cdp: &Cdp, sel: &Selectors) -> Result<()> {
    let url = cdp.eval("location.href").await?;
    if !url.as_str().unwrap_or("").contains("tiktok.com/messages") {
        cdp.call("Page.navigate", json!({ "url": MESSAGES_URL })).await?;
        sleep(Duration::from_secs(3)).await;
    }
    let probe = js(sel, "if (items().length) return 'ready'; if (isLoggedOut()) return 'login'; return 'wait';");
    let mut login_seen = 0;
    for _ in 0..30 {
        match cdp.eval(&probe).await?.as_str() {
            Some("ready") => return Ok(()),
            Some("login") => {
                login_seen += 1;
                if login_seen >= 3 {
                    bail!(NOT_LOGGED_IN);
                }
            }
            _ => {}
        }
        sleep(Duration::from_secs(1)).await;
    }
    bail!("Не удалось найти список чатов на странице. Запусти «Самопроверку» — она покажет, что сломалось.")
}

/// Собирает список чатов (прокручивая список, чтобы подгрузить всех).
pub async fn list_chats(cdp: &Cdp, sel: &Selectors) -> Result<Vec<Chat>> {
    let v = cdp
        .eval(&js(
            sel,
            r#"const seen = new Map();
const collect = () => {
  for (const el of items()) {
    const name = nameOf(el);
    if (!name || seen.has(name)) continue;
    const img = el.querySelector('img');
    const l = S.chatLast ? el.querySelector(S.chatLast) : null;
    const tm = S.chatTime ? el.querySelector(S.chatTime) : null;
    let last = '';
    if (l) {
      const c = l.cloneNode(true);
      c.querySelectorAll('[class*="InfoTime"],[class*="Dot"]').forEach(x => x.remove());
      last = c.textContent.trim().replace(/\s*\d{1,2}:\d{2}$/, '');
    }
    seen.set(name, { name, avatar: img ? img.src : '', last, time: tm ? tm.textContent.trim() : '' });
  }
};
// Список виртуальный: показывает только чаты рядом с видимой областью,
// поэтому прокручиваем пошагово, собирая имена на каждом шаге.
for (let i = 0; i < 20 && !items().some(nameOf); i++) await wait(500);
await wait(800);
collect();
const box = scrollBox();
if (box) {
  box.scrollTop = 0; await wait(500); collect();
  let still = 0;
  for (let i = 0; i < 200 && still < 3; i++) {
    const prev = box.scrollTop;
    box.scrollTop = prev + box.clientHeight * 0.7;
    await wait(700);
    collect();
    still = box.scrollTop === prev ? still + 1 : 0;
  }
  box.scrollTop = 0;
}
return [...seen.values()];"#,
        ))
        .await?;
    Ok(serde_json::from_value(v).unwrap_or_default())
}

/// Находит чат в списке, открывает его и проверяет, что открылся нужный.
async fn open_chat(cdp: &Cdp, sel: &Selectors, name: &str) -> Result<()> {
    let pt = cdp
        .eval(&js(
            sel,
            &format!(
                "const el = await findChat({}); if (!el) return null; el.scrollIntoView({{block:'center'}}); await wait(300); return center(el);",
                json!(name)
            ),
        ))
        .await?;
    let (x, y) = point(&pt).ok_or_else(|| anyhow!("Чат не найден в списке"))?;
    cdp.click(x, y).await?;
    sleep(Duration::from_millis(2000)).await;

    let opened = cdp
        .eval(&js(
            sel,
            &format!(
                r#"const want = {};
const h = S.chatHeader && document.querySelector(S.chatHeader);
if (h) return h.textContent.trim() === want;
return [...document.querySelectorAll('p,span,a,h1,h2,h3,div')].some(e =>
  !e.closest(S.chatItem) && e.children.length === 0 && e.textContent.trim() === want);"#,
                json!(name)
            ),
        ))
        .await?;
    if opened.as_bool() != Some(true) {
        bail!("Не удалось убедиться, что открыт чат именно с этим человеком");
    }
    Ok(())
}

/// Открывает чат с человеком и отправляет ему сообщение.
pub async fn send_to(cdp: &Cdp, sel: &Selectors, name: &str, text: &str) -> Result<()> {
    open_chat(cdp, sel, name).await.map_err(|e| anyhow!("{e} — отправка отменена"))?;

    let ip = cdp
        .eval(&js(
            sel,
            "const ed = editor(); if (!ed) return null; ed.focus(); const r = ed.getBoundingClientRect(); return { x: r.left + Math.min(40, r.width / 2), y: r.top + r.height / 2 };",
        ))
        .await?;
    let (x, y) = point(&ip).ok_or_else(|| anyhow!("Не найдено поле ввода сообщения"))?;
    cdp.click(x, y).await?;
    sleep(Duration::from_millis(300)).await;
    cdp.call("Input.insertText", json!({ "text": text })).await?;
    sleep(Duration::from_millis(600)).await;
    cdp.press_enter().await?;
    sleep(Duration::from_millis(1500)).await;

    // Поле очистилось — значит, отправилось. Если нет — пробуем кнопку отправки.
    let left_js = js(sel, "const ed = editor(); return ed ? ed.textContent.trim() : '';");
    if !cdp.eval(&left_js).await?.as_str().unwrap_or("").is_empty() {
        let b = cdp
            .eval(&js(sel, "const b = S.sendButton && document.querySelector(S.sendButton); return b ? center(b) : null;"))
            .await?;
        if let Some((x, y)) = point(&b) {
            cdp.click(x, y).await?;
            sleep(Duration::from_millis(1500)).await;
        }
        if !cdp.eval(&left_js).await?.as_str().unwrap_or("").is_empty() {
            bail!("Сообщение не отправилось (текст остался в поле ввода)");
        }
    }
    Ok(())
}

/// Открывает чат и смотрит, есть ли сегодня входящие сообщения от человека.
pub async fn replied_today(cdp: &Cdp, sel: &Selectors, name: &str) -> Result<bool> {
    open_chat(cdp, sel, name).await?;
    sleep(Duration::from_millis(800)).await;
    let v = cdp
        .eval(&js(
            sel,
            r#"const list = document.querySelector(S.messageList);
if (!list) return null;
const me = (S.myProfile && document.querySelector(S.myProfile)?.getAttribute('href')) || '';
const lr = list.getBoundingClientRect();
const now = new Date(); const nowMin = now.getHours() * 60 + now.getMinutes();
const isToday = t => { const m = /^(\d{1,2}):(\d{2})$/.exec(t.trim()); return !!m && (+m[1]) * 60 + (+m[2]) <= nowMin; };
let today = false, replied = false;
for (const el of list.querySelectorAll(S.messageItem + ',' + S.timeSeparator)) {
  if (el.matches(S.timeSeparator)) { today = isToday(el.textContent); continue; }
  if (!today) continue;
  const href = el.querySelector('a[href*="/@"]')?.getAttribute('href') || '';
  let own = !!me && href === me;
  if (!own && !href) {
    const a = S.messageAvatar && el.querySelector(S.messageAvatar);
    if (a) { const r = a.getBoundingClientRect(); own = (r.left - lr.left) > lr.width / 2; }
  }
  if (!own) { replied = true; break; }
}
return replied;"#,
        ))
        .await?;
    v.as_bool().ok_or_else(|| anyhow!("Не найден список сообщений в чате"))
}

/// Самопроверка страницы: ничего не открывает и не отправляет, только смотрит.
pub async fn probe(cdp: &Cdp, sel: &Selectors) -> Result<Value> {
    let url = cdp.eval("location.href").await?;
    if !url.as_str().unwrap_or("").contains("tiktok.com/messages") {
        cdp.call("Page.navigate", json!({ "url": MESSAGES_URL })).await?;
        sleep(Duration::from_secs(3)).await;
    }
    cdp.eval(&js(
        sel,
        r#"for (let i = 0; i < 15 && !items().length && !isLoggedOut(); i++) await wait(1000);
// Имена дорисовываются чуть позже самих строк — ждём, пока догрузятся.
for (let i = 0; i < 10 && items().some(e => !nameOf(e)); i++) await wait(500);
const its = items();
return {
  loggedIn: !isLoggedOut(),
  chats: its.length,
  names: its.filter(e => nameOf(e)).length,
  times: S.chatTime ? its.filter(e => e.querySelector(S.chatTime)).length : 0,
  chatOpen: !!(S.chatHeader && document.querySelector(S.chatHeader)),
  editor: !!editor(),
  messageList: !!(S.messageList && document.querySelector(S.messageList)),
  me: !!(S.myProfile && document.querySelector(S.myProfile)),
};"#,
    ))
    .await
}
