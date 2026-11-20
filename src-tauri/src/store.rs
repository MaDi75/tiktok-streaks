use serde::{Deserialize, Serialize};
use std::{collections::HashMap, fs, path::PathBuf, sync::Mutex};

#[derive(Serialize, Deserialize, Clone, Default, Debug)]
#[serde(default, rename_all = "camelCase")]
pub struct Chat {
    pub name: String,
    pub avatar: String,
    pub last: String,
    /// Время/дата последнего сообщения из списка чатов: "13:20" (сегодня) или "16.11.2026".
    pub time: String,
}

/// CSS-селекторы сайта TikTok. Вынесены в настройки: если TikTok поменяет вёрстку,
/// их можно поправить без пересборки приложения.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default, rename_all = "camelCase")]
pub struct Selectors {
    pub chat_item: String,
    pub chat_name: String,
    pub chat_last: String,
    pub chat_time: String,
    pub chat_header: String,
    pub input: String,
    pub send_button: String,
    pub login_button: String,
    pub message_list: String,
    pub message_item: String,
    pub time_separator: String,
    pub message_avatar: String,
    pub my_profile: String,
}

/// Версия селекторов по умолчанию; при её смене старые сохранённые селекторы заменяются.
pub const SELECTORS_VERSION: u32 = 3;

impl Default for Selectors {
    fn default() -> Self {
        Self {
            chat_item: r#"[data-e2e="dm-new-conversation-item"]"#.into(),
            chat_name: r#"[data-e2e="dm-new-conversation-nickname"]"#.into(),
            chat_last: r#"p[class*="InfoExtract"]"#.into(),
            chat_time: r#"[class*="InfoTime"]"#.into(),
            chat_header: r#"[data-e2e="dm-new-chat-nickname"]"#.into(),
            input: r#"[data-e2e="dm-new-input-editor"]"#.into(),
            send_button: r#"[data-e2e*="send"]"#.into(),
            login_button: r#"[data-e2e="top-login-button"]"#.into(),
            message_list: r#"[data-e2e="dm-new-message-list"]"#.into(),
            message_item: r#"[data-e2e="dm-new-chat-item"]"#.into(),
            time_separator: r#"[data-e2e="dm-new-time-separator"]"#.into(),
            message_avatar: r#"[data-e2e="chat-avatar"]"#.into(),
            my_profile: r#"[data-e2e="nav-profile"] a, a[data-e2e="nav-profile"]"#.into(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default, rename_all = "camelCase")]
pub struct Config {
    /// Последний загруженный список чатов (кэш для интерфейса).
    pub chats: Vec<Chat>,
    /// Имена людей, с кем есть серия.
    pub marked: Vec<String>,
    /// Что отправлять по умолчанию.
    pub default_message: String,
    /// Своё сообщение для конкретного человека (если пусто — общее).
    pub per_person: HashMap<String, String>,
    pub auto_enabled: bool,
    /// "HH:MM"
    pub auto_time: String,
    pub skip_if_sent_today: bool,
    /// имя -> дата последней успешной отправки "YYYY-MM-DD"
    pub history: HashMap<String, String>,
    pub last_auto_run: String,

    // --- защита серии ---
    pub guard_enabled: bool,
    pub guard_time: String,
    /// имя -> дата, когда человек точно писал мне
    pub replied: HashMap<String, String>,
    /// дата последней проверки ответов
    pub replies_checked: String,
    pub last_guard_run: String,

    // --- браузер ---
    pub hide_browser: bool,

    // --- Telegram ---
    pub telegram_token: String,
    pub telegram_chat_id: i64,

    pub selectors: Selectors,
    pub selectors_version: u32,

    // --- анонимная статистика (opt-in, по умолчанию выключена) ---
    /// Отправлять ли анонимную статистику использования.
    pub telemetry_enabled: bool,
    /// Адрес сервера-приёмника. Пусто — ничего не отправляется, даже если включено.
    pub telemetry_endpoint: String,
    /// Случайный анонимный идентификатор этой установки (создаётся один раз).
    pub install_id: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            chats: vec![],
            marked: vec![],
            default_message: "🔥".into(),
            per_person: HashMap::new(),
            auto_enabled: true,
            auto_time: "20:00".into(),
            skip_if_sent_today: true,
            history: HashMap::new(),
            last_auto_run: String::new(),
            guard_enabled: true,
            guard_time: "21:00".into(),
            replied: HashMap::new(),
            replies_checked: String::new(),
            last_guard_run: String::new(),
            hide_browser: true,
            telegram_token: String::new(),
            telegram_chat_id: 0,
            selectors: Selectors::default(),
            selectors_version: SELECTORS_VERSION,
            telemetry_enabled: false,
            telemetry_endpoint: String::new(),
            install_id: String::new(),
        }
    }
}

pub struct Store {
    path: PathBuf,
    cfg: Mutex<Config>,
}

impl Store {
    pub fn load(path: PathBuf) -> Self {
        let raw = fs::read_to_string(&path).ok();
        let mut cfg: Config = raw.as_deref().and_then(|s| serde_json::from_str(s).ok()).unwrap_or_default();
        // Новые поля, которых не было в старом config.json, должны получить значения по умолчанию
        // (serde(default) берёт их из Default, а не нули — это уже так), а старые селекторы — обновиться.
        if cfg.selectors_version < SELECTORS_VERSION {
            cfg.selectors = Selectors::default();
            cfg.selectors_version = SELECTORS_VERSION;
        }
        Self { path, cfg: Mutex::new(cfg) }
    }

    pub fn get(&self) -> Config {
        self.cfg.lock().unwrap().clone()
    }

    pub fn update(&self, f: impl FnOnce(&mut Config)) {
        let mut c = self.cfg.lock().unwrap();
        f(&mut c);
        if let Ok(s) = serde_json::to_string_pretty(&*c) {
            let _ = fs::write(&self.path, s);
        }
    }
}

pub fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

/// "13:20" из списка чатов/разделителя относится к сегодняшнему дню?
/// TikTok пишет только время для сегодняшних сообщений и дату для более старых.
/// Время позже текущего — это вчерашний вечер.
pub fn is_today_time(t: &str) -> bool {
    let t = t.trim();
    let Some((h, m)) = t.split_once(':') else { return false };
    let (Ok(h), Ok(m)) = (h.parse::<u32>(), m.parse::<u32>()) else { return false };
    use chrono::Timelike;
    let now = chrono::Local::now();
    h * 60 + m <= now.hour() * 60 + now.minute()
}
