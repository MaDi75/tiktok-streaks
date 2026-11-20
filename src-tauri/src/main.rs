// TikTok Streaks — помощник для поддержания серий в личных сообщениях TikTok.
// © 2026 MaDi75 — https://github.com/MaDi75/tiktok-streaks
// Лицензия MIT (см. файл LICENSE). При копировании сохраняйте указание автора.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod browser;
mod scheduler;
mod store;
mod telegram;
mod tiktok;

/// Подпись автора — вшивается в бинарник для доказательства авторства при копировании.
/// `#[used]` не даёт линкеру её выкинуть. Не удалять.
#[used]
static BUILD_SIGNATURE: &[u8] =
    b"TikTokStreaks::author=MaDi75::repo=github.com/MaDi75/tiktok-streaks";

use rand::Rng;
use serde::Serialize;
use std::{path::PathBuf, time::Duration};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, State, WindowEvent,
};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};
use tauri_plugin_notification::NotificationExt;

pub struct AppState {
    pub store: store::Store,
    pub busy: tokio::sync::Mutex<()>,
    pub profile_dir: PathBuf,
    /// Одноразовый код привязки Telegram (пусто — привязка не ожидается).
    pub pair_code: std::sync::Mutex<String>,
}

#[derive(Serialize, Clone)]
struct Progress {
    name: String,
    status: String,
    detail: String,
}

#[derive(Serialize, Clone, Default)]
pub struct Summary {
    sent: u32,
    skipped: u32,
    failed: u32,
}

#[derive(Serialize, Clone, Default)]
pub struct CheckResult {
    replied: Vec<String>,
    missing: Vec<String>,
    unknown: Vec<String>,
}

#[derive(Serialize, Clone)]
struct DiagItem {
    title: String,
    /// true — ок, false — проблема, null — информация/пропущено
    ok: Option<bool>,
    detail: String,
}

fn e2s<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

fn emit(app: &AppHandle, name: &str, status: &str, detail: &str) {
    let _ = app.emit(
        "send-progress",
        Progress { name: name.into(), status: status.into(), detail: detail.into() },
    );
}

/// Уведомление Windows + сообщение в Telegram (если привязан).
pub fn notify(app: &AppHandle, body: &str) {
    let _ = app.notification().builder().title("TikTok Streaks").body(body).show();
    telegram::notify(app, body);
}

pub fn notify_summary(app: &AppHandle, s: &Summary) {
    let mut t = format!("🔥 Отправлено: {}", s.sent);
    if s.skipped > 0 {
        t += &format!(", уже было сегодня: {}", s.skipped);
    }
    if s.failed > 0 {
        t += &format!(", ошибок: {} — открой приложение", s.failed);
    }
    notify(app, &t);
}

/// Подключиться к браузеру, спрятать его (если надо) и открыть страницу сообщений.
/// Если нужен вход — показывает окно браузера.
async fn open_tiktok(state: &AppState, cfg: &store::Config) -> Result<browser::Cdp, String> {
    let cdp = browser::ensure(&state.profile_dir, cfg.hide_browser).await.map_err(e2s)?;
    if cfg.hide_browser {
        cdp.set_visible(false).await.ok();
    }
    if let Err(e) = tiktok::ensure_messages_page(&cdp, &cfg.selectors).await {
        if e.to_string() == tiktok::NOT_LOGGED_IN {
            cdp.set_visible(true).await.ok();
        }
        return Err(e2s(e));
    }
    Ok(cdp)
}

/// Новый список чатов + те из старого, которых в новом не оказалось (чтобы никто не «пропадал»).
fn merge_chats(state: &AppState, fresh: &[store::Chat]) {
    state.store.update(|c| {
        let mut merged = fresh.to_vec();
        for old in &c.chats {
            if !merged.iter().any(|n| n.name == old.name) {
                merged.push(old.clone());
            }
        }
        c.chats = merged;
    });
}

async fn pause(min_ms: u64, max_ms: u64) {
    let ms = rand::thread_rng().gen_range(min_ms..max_ms);
    tokio::time::sleep(Duration::from_millis(ms)).await;
}

/// Отправка всем отмеченным (или одному человеку, если указан `only`).
pub async fn run_send(app: &AppHandle, only: Option<String>) -> Result<Summary, String> {
    let state = app.state::<AppState>();
    let _guard = state.busy.try_lock().map_err(|_| "Приложение уже занято отправкой или проверкой".to_string())?;
    let cfg = state.store.get();
    let today = store::today();
    let single = only.is_some();
    let targets: Vec<String> = match only {
        Some(n) => vec![n],
        None => cfg.marked.clone(),
    };
    if targets.is_empty() {
        return Err("Никто не отмечен. Нажми 🔥 у людей, с кем у тебя серия.".into());
    }

    let _ = app.emit("send-started", targets.len());
    let cdp = match open_tiktok(&state, &cfg).await {
        Ok(c) => c,
        Err(e) => {
            let _ = app.emit("send-finished", Summary::default());
            return Err(e);
        }
    };

    let mut sum = Summary::default();
    let total = targets.len();
    for (i, name) in targets.iter().enumerate() {
        let done_today = cfg.history.get(name).map(|d| d == &today).unwrap_or(false);
        if !single && cfg.skip_if_sent_today && done_today {
            sum.skipped += 1;
            emit(app, name, "skipped", "уже отправлено сегодня");
            continue;
        }
        emit(app, name, "sending", "");
        let msg = cfg
            .per_person
            .get(name)
            .filter(|s| !s.trim().is_empty())
            .cloned()
            .unwrap_or_else(|| cfg.default_message.clone());
        match tiktok::send_to(&cdp, &cfg.selectors, name, &msg).await {
            Ok(()) => {
                sum.sent += 1;
                state.store.update(|c| {
                    c.history.insert(name.clone(), today.clone());
                });
                emit(app, name, "ok", &msg);
            }
            Err(e) => {
                sum.failed += 1;
                emit(app, name, "error", &e.to_string());
            }
        }
        if i + 1 < total {
            // Случайная пауза, чтобы не выглядеть как бот.
            pause(3000, 10000).await;
        }
    }
    let _ = app.emit("send-finished", sum.clone());
    Ok(sum)
}

/// Защита серии: кто из отмеченных писал мне сегодня.
/// `report_all` — прислать полный отчёт (ручная проверка / команда /check), иначе — только предупреждение.
pub async fn run_check(app: &AppHandle, report_all: bool) -> Result<CheckResult, String> {
    let state = app.state::<AppState>();
    let _guard = state.busy.try_lock().map_err(|_| "Приложение уже занято отправкой или проверкой".to_string())?;
    let cfg = state.store.get();
    if cfg.marked.is_empty() {
        return Err("Никто не отмечен 🔥".into());
    }
    let today = store::today();
    let _ = app.emit("check-started", ());
    let result = async {
        let cdp = open_tiktok(&state, &cfg).await?;
        let chats = tiktok::list_chats(&cdp, &cfg.selectors).await.map_err(e2s)?;
        if !chats.is_empty() {
            merge_chats(&state, &chats);
        }
        let mut res = CheckResult::default();
        for name in &cfg.marked {
            if cfg.replied.get(name) == Some(&today) {
                res.replied.push(name.clone());
                continue;
            }
            // Если последнее сообщение в чате не сегодняшнее — никто сегодня не писал, чат не открываем.
            let time = chats.iter().find(|c| &c.name == name).map(|c| c.time.clone());
            if let Some(t) = &time {
                if !t.is_empty() && !store::is_today_time(t) {
                    res.missing.push(name.clone());
                    continue;
                }
            }
            match tiktok::replied_today(&cdp, &cfg.selectors, name).await {
                Ok(true) => {
                    state.store.update(|c| {
                        c.replied.insert(name.clone(), today.clone());
                    });
                    res.replied.push(name.clone());
                }
                Ok(false) => res.missing.push(name.clone()),
                Err(_) => res.unknown.push(name.clone()),
            }
            pause(1500, 4000).await;
        }
        state.store.update(|c| c.replies_checked = today.clone());
        Ok::<_, String>(res)
    }
    .await;
    let _ = app.emit("check-finished", ());
    let res = result?;

    if !res.missing.is_empty() {
        notify(app, &format!("⚠️ Серия под угрозой! Сегодня ещё не ответили: {}", res.missing.join(", ")));
    }
    if report_all {
        let mut t = format!("💬 Ответили сегодня: {}", if res.replied.is_empty() { "никто".into() } else { res.replied.join(", ") });
        if !res.unknown.is_empty() {
            t += &format!("\n❓ Не удалось проверить: {}", res.unknown.join(", "));
        }
        if res.missing.is_empty() {
            t += "\nВсе серии в безопасности 🔥";
        }
        notify(app, &t);
    }
    Ok(res)
}

// ------------------------------------------------------------------ команды

#[tauri::command]
fn get_config(state: State<AppState>) -> store::Config {
    state.store.get()
}

#[tauri::command]
fn save_config(state: State<AppState>, cfg: store::Config) {
    // История и служебные поля не перезаписываются из интерфейса.
    state.store.update(|c| {
        if c.telegram_token != cfg.telegram_token {
            c.telegram_chat_id = 0; // новый бот — нужна новая привязка
        }
        c.marked = cfg.marked;
        c.default_message = cfg.default_message;
        c.per_person = cfg.per_person;
        c.auto_enabled = cfg.auto_enabled;
        c.auto_time = cfg.auto_time;
        c.skip_if_sent_today = cfg.skip_if_sent_today;
        c.guard_enabled = cfg.guard_enabled;
        c.guard_time = cfg.guard_time;
        c.hide_browser = cfg.hide_browser;
        c.telegram_token = cfg.telegram_token.trim().to_string();
        c.selectors = cfg.selectors;
    });
}

/// Версия приложения — для карточки «О программе».
#[tauri::command]
fn app_version(app: AppHandle) -> String {
    app.package_info().version.to_string()
}

#[tauri::command]
async fn open_browser(state: State<'_, AppState>) -> Result<(), String> {
    let cdp = browser::ensure(&state.profile_dir, false).await.map_err(e2s)?;
    let url = cdp.eval("location.href").await.map_err(e2s)?;
    if !url.as_str().unwrap_or("").contains("tiktok.com") {
        cdp.call("Page.navigate", serde_json::json!({ "url": tiktok::MESSAGES_URL })).await.map_err(e2s)?;
    }
    cdp.set_visible(true).await.map_err(e2s)
}

#[tauri::command]
async fn hide_browser_now(state: State<'_, AppState>) -> Result<(), String> {
    if !browser::is_running().await {
        return Ok(());
    }
    let cdp = browser::ensure(&state.profile_dir, true).await.map_err(e2s)?;
    cdp.set_visible(false).await.map_err(e2s)
}

#[tauri::command]
async fn refresh_chats(state: State<'_, AppState>) -> Result<Vec<store::Chat>, String> {
    let _guard = state.busy.try_lock().map_err(|_| "Подожди, приложение занято".to_string())?;
    let cfg = state.store.get();
    let cdp = open_tiktok(&state, &cfg).await?;
    let chats = tiktok::list_chats(&cdp, &cfg.selectors).await.map_err(e2s)?;
    if chats.is_empty() {
        return Err("Список чатов пуст. Запусти «Самопроверку».".into());
    }
    merge_chats(&state, &chats);
    Ok(state.store.get().chats)
}

#[tauri::command]
async fn send_all(app: AppHandle) -> Result<Summary, String> {
    let s = run_send(&app, None).await?;
    notify_summary(&app, &s);
    Ok(s)
}

#[tauri::command]
async fn send_one(app: AppHandle, name: String) -> Result<Summary, String> {
    run_send(&app, Some(name)).await
}

#[tauri::command]
async fn check_replies(app: AppHandle) -> Result<CheckResult, String> {
    run_check(&app, false).await
}

#[tauri::command]
async fn diagnose(state: State<'_, AppState>) -> Result<Vec<DiagItem>, String> {
    let _guard = state.busy.try_lock().map_err(|_| "Подожди, приложение занято".to_string())?;
    let cfg = state.store.get();
    let mut out = vec![];
    let mut add = |title: &str, ok: Option<bool>, detail: String| {
        out.push(DiagItem { title: title.into(), ok, detail })
    };

    match browser::find_browser() {
        Some(p) => add("Браузер найден", Some(true), p.display().to_string()),
        None => {
            add("Браузер найден", Some(false), "Установи Microsoft Edge или Google Chrome".into());
            return Ok(out);
        }
    }
    let cdp = match browser::ensure(&state.profile_dir, cfg.hide_browser).await {
        Ok(c) => {
            add("Связь с браузером", Some(true), format!("порт {}", browser::PORT));
            c
        }
        Err(e) => {
            add("Связь с браузером", Some(false), format!("{e}. Закрой все окна этого Edge и попробуй снова."));
            return Ok(out);
        }
    };
    let p = match tiktok::probe(&cdp, &cfg.selectors).await {
        Ok(p) => p,
        Err(e) => {
            add("Страница TikTok", Some(false), e.to_string());
            return Ok(out);
        }
    };
    let n = |k: &str| p[k].as_u64().unwrap_or(0);
    let b = |k: &str| p[k].as_bool().unwrap_or(false);

    if !b("loggedIn") {
        cdp.set_visible(true).await.ok();
        add("Вход в аккаунт", Some(false), "Не выполнен — окно браузера показано, войди в TikTok".into());
        return Ok(out);
    }
    add("Вход в аккаунт", Some(true), String::new());

    let chats = n("chats");
    add(
        "Список чатов",
        Some(chats > 0),
        if chats > 0 { format!("видно чатов: {chats}") } else { "не найден — селектор chatItem устарел".into() },
    );
    if chats > 0 {
        let names = n("names");
        add("Имена в чатах", Some(names == chats), format!("{names} из {chats}"));
        let times = n("times");
        add(
            "Время сообщений",
            Some(times > 0),
            if times > 0 { "нужно для защиты серии".into() } else { "не читается — селектор chatTime устарел".into() },
        );
    }
    add("Твой профиль", Some(b("me")), if b("me") { "нужно, чтобы отличать свои сообщения".into() } else { "селектор myProfile устарел".into() });
    if b("chatOpen") {
        add("Поле ввода сообщения", Some(b("editor")), if b("editor") { String::new() } else { "селектор input устарел".into() });
        add("Список сообщений в чате", Some(b("messageList")), String::new());
    } else {
        add("Поле ввода сообщения", None, "проверится при первой отправке (сейчас ни один чат не открыт)".into());
    }

    let missing: Vec<_> = cfg.marked.iter().filter(|m| !cfg.chats.iter().any(|c| &c.name == *m)).cloned().collect();
    if cfg.marked.is_empty() {
        add("Отмеченные люди", None, "никто не отмечен 🔥".into());
    } else if missing.is_empty() {
        add("Отмеченные люди", Some(true), format!("все {} есть в списке", cfg.marked.len()));
    } else {
        add("Отмеченные люди", Some(false), format!("нет в списке (сменили ник?): {}", missing.join(", ")));
    }

    if !cfg.telegram_token.is_empty() {
        match telegram::get_me(&cfg.telegram_token).await {
            Ok(u) => add(
                "Telegram-бот",
                Some(cfg.telegram_chat_id != 0),
                if cfg.telegram_chat_id != 0 { format!("{u}, привязан") } else { format!("{u}, но ещё не привязан") },
            ),
            Err(e) => add("Telegram-бот", Some(false), e.to_string()),
        }
    }
    if cfg.hide_browser {
        cdp.set_visible(false).await.ok();
    }
    Ok(out)
}

#[tauri::command]
async fn tg_test(token: String) -> Result<String, String> {
    telegram::get_me(token.trim()).await.map_err(e2s)
}

#[tauri::command]
fn tg_pair_code(state: State<AppState>) -> String {
    let code = format!("{:06}", rand::thread_rng().gen_range(0..1_000_000));
    *state.pair_code.lock().unwrap() = code.clone();
    code
}

#[tauri::command]
fn tg_unlink(state: State<AppState>) {
    state.store.update(|c| c.telegram_chat_id = 0);
}

#[tauri::command]
fn get_autostart(app: AppHandle) -> bool {
    app.autolaunch().is_enabled().unwrap_or(false)
}

#[tauri::command]
fn set_autostart(app: AppHandle, enabled: bool) -> Result<(), String> {
    let a = app.autolaunch();
    if enabled { a.enable() } else { a.disable() }.map_err(e2s)
}

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

fn spawn_task(app: &AppHandle, check: bool) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if check {
            if let Err(e) = run_check(&app, true).await {
                notify(&app, &e);
            }
        } else {
            match run_send(&app, None).await {
                Ok(s) => notify_summary(&app, &s),
                Err(e) => notify(&app, &e),
            }
        }
    });
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, Some(vec!["--minimized"])))
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            app.manage(AppState {
                store: store::Store::load(dir.join("config.json")),
                busy: tokio::sync::Mutex::new(()),
                profile_dir: dir.join("browser-profile"),
                pair_code: std::sync::Mutex::new(String::new()),
            });

            let open_i = MenuItem::with_id(app, "open", "Открыть", true, None::<&str>)?;
            let send_i = MenuItem::with_id(app, "send", "Отправить сейчас", true, None::<&str>)?;
            let check_i = MenuItem::with_id(app, "check", "Проверить ответы", true, None::<&str>)?;
            let quit_i = MenuItem::with_id(app, "quit", "Выход", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open_i, &send_i, &check_i, &quit_i])?;
            TrayIconBuilder::new()
                .icon(app.default_window_icon().unwrap().clone())
                .tooltip("TikTok Streaks")
                .menu(&menu)
                .on_menu_event(|app, ev| match ev.id.as_ref() {
                    "open" => show_main(app),
                    "send" => spawn_task(app, false),
                    "check" => spawn_task(app, true),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, ev| {
                    if let TrayIconEvent::DoubleClick { button: MouseButton::Left, .. } = ev {
                        show_main(tray.app_handle());
                    }
                })
                .build(app)?;

            if !std::env::args().any(|a| a == "--minimized") {
                show_main(app.handle());
            }
            scheduler::start(app.handle().clone());
            telegram::start(app.handle().clone());
            Ok(())
        })
        .on_window_event(|w, ev| {
            // Крестик сворачивает в трей, а не закрывает приложение.
            if let WindowEvent::CloseRequested { api, .. } = ev {
                let _ = w.hide();
                api.prevent_close();
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_config,
            save_config,
            open_browser,
            hide_browser_now,
            refresh_chats,
            send_all,
            send_one,
            check_replies,
            diagnose,
            tg_test,
            tg_pair_code,
            tg_unlink,
            get_autostart,
            set_autostart,
            app_version
        ])
        .run(tauri::generate_context!())
        .expect("ошибка запуска приложения");
}
