//! Telegram-бот: уведомления и команды /status, /send, /check.
//! Принимает команды только из одного чата, привязанного кодом из приложения.

use crate::{store, AppState};
use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tokio::time::sleep;

fn api(token: &str, method: &str) -> String {
    format!("https://api.telegram.org/bot{token}/{method}")
}

fn client(secs: u64) -> reqwest::Client {
    reqwest::Client::builder().timeout(Duration::from_secs(secs)).build().unwrap()
}

/// Проверка токена: возвращает @имя бота.
pub async fn get_me(token: &str) -> Result<String> {
    let v: Value = client(15).get(api(token, "getMe")).send().await?.json().await?;
    if v["ok"].as_bool() != Some(true) {
        return Err(anyhow!("Telegram отклонил токен: {}", v["description"].as_str().unwrap_or("?")));
    }
    Ok(format!("@{}", v["result"]["username"].as_str().unwrap_or("?")))
}

pub async fn send(token: &str, chat_id: i64, text: &str) -> Result<()> {
    client(20)
        .post(api(token, "sendMessage"))
        .json(&json!({ "chat_id": chat_id, "text": text }))
        .send()
        .await?;
    Ok(())
}

/// Отправить сообщение в привязанный чат, если бот настроен.
pub fn notify(app: &AppHandle, text: &str) {
    let cfg = app.state::<AppState>().store.get();
    if cfg.telegram_token.is_empty() || cfg.telegram_chat_id == 0 {
        return;
    }
    let text = text.to_string();
    tauri::async_runtime::spawn(async move {
        let _ = send(&cfg.telegram_token, cfg.telegram_chat_id, &text).await;
    });
}

pub fn status_text(cfg: &store::Config) -> String {
    if cfg.marked.is_empty() {
        return "Никто не отмечен 🔥 в приложении.".into();
    }
    let today = store::today();
    let checked = cfg.replies_checked == today;
    let mut s = String::from("Серии на сегодня:\n");
    for n in &cfg.marked {
        let sent = cfg.history.get(n) == Some(&today);
        let replied = cfg.replied.get(n) == Some(&today);
        let r = if replied { "💬 ответил" } else if checked { "⚠️ не ответил" } else { "· ответ не проверен" };
        s += &format!("{} {n} — {}, {r}\n", if sent { "✅" } else { "⏳" }, if sent { "отправлено" } else { "не отправлено" });
    }
    s
}

const HELP: &str = "Команды:\n/status — кому отправлено и кто ответил\n/send — отправить всем отмеченным\n/send имя — отправить одному\n/check — проверить, кто ответил сегодня";

async fn handle(app: &AppHandle, token: &str, upd: &Value) {
    let msg = &upd["message"];
    let Some(chat_id) = msg["chat"]["id"].as_i64() else { return };
    let text = msg["text"].as_str().unwrap_or("").trim().to_string();
    let state = app.state::<AppState>();
    let cfg = state.store.get();

    if let Some(rest) = text.strip_prefix("/start") {
        let code = rest.trim();
        let expected = state.pair_code.lock().unwrap().clone();
        if !expected.is_empty() && code == expected {
            state.store.update(|c| c.telegram_chat_id = chat_id);
            *state.pair_code.lock().unwrap() = String::new();
            let _ = app.emit("telegram-paired", ());
            let _ = send(token, chat_id, &format!("Привязано ✅\n\n{HELP}")).await;
        } else if chat_id == cfg.telegram_chat_id {
            let _ = send(token, chat_id, HELP).await;
        } else {
            let _ = send(token, chat_id, "Отправь код из приложения TikTok Streaks: /start КОД").await;
        }
        return;
    }
    // Всё остальное — только от привязанного чата.
    if cfg.telegram_chat_id == 0 || chat_id != cfg.telegram_chat_id {
        return;
    }

    if text == "/status" {
        let _ = send(token, chat_id, &status_text(&cfg)).await;
    } else if let Some(rest) = text.strip_prefix("/send") {
        let name = rest.trim();
        let only = if name.is_empty() { None } else { Some(name.to_string()) };
        let _ = send(token, chat_id, "Отправляю… итог пришлю сюда.").await;
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            match crate::run_send(&app, only).await {
                Ok(s) => crate::notify_summary(&app, &s),
                Err(e) => crate::notify(&app, &format!("Не получилось: {e}")),
            }
        });
    } else if text == "/check" {
        let _ = send(token, chat_id, "Проверяю ответы… это займёт пару минут.").await;
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(e) = crate::run_check(&app, true).await {
                crate::notify(&app, &format!("Проверка не удалась: {e}"));
            }
        });
    } else {
        let _ = send(token, chat_id, HELP).await;
    }
}

/// Фоновый цикл long polling.
pub fn start(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let http = client(45);
        let mut offset: i64 = 0;
        let mut last_token = String::new();
        loop {
            let token = app.state::<AppState>().store.get().telegram_token;
            if token.is_empty() {
                sleep(Duration::from_secs(5)).await;
                continue;
            }
            if token != last_token {
                offset = 0;
                last_token = token.clone();
            }
            let resp = http
                .get(api(&token, "getUpdates"))
                .query(&[("timeout", "30".to_string()), ("offset", offset.to_string())])
                .send()
                .await;
            let v: Value = match resp {
                Ok(r) => r.json().await.unwrap_or(Value::Null),
                Err(_) => {
                    sleep(Duration::from_secs(10)).await;
                    continue;
                }
            };
            if v["ok"].as_bool() != Some(true) {
                sleep(Duration::from_secs(30)).await;
                continue;
            }
            for upd in v["result"].as_array().cloned().unwrap_or_default() {
                if let Some(id) = upd["update_id"].as_i64() {
                    offset = id + 1;
                }
                handle(&app, &token, &upd).await;
            }
        }
    });
}
