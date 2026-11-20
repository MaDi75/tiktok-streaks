// Анонимная статистика использования — ЧЕСТНАЯ и ДОБРОВОЛЬНАЯ.
//
// Ничего не отправляется, пока пользователь сам не включит переключатель
// в настройках И не укажет адрес сервера. По умолчанию выключено.
//
// Что отправляется (и только это):
//   - version    — версия приложения,
//   - os         — тип ОС ("windows"),
//   - installId  — случайный анонимный идентификатор установки.
//
// IP-адрес сервер видит сам, как при любом сетевом соединении (это и есть
// способ понять географию/количество пользователей). Личные данные,
// сообщения, имена контактов и аккаунт TikTok НЕ собираются и НЕ отправляются.

use crate::AppState;
use serde_json::json;
use std::time::Duration;
use tauri::{AppHandle, Manager};

/// Период между отправками — раз в сутки.
const INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

pub fn start(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            ping(&app).await;
            tokio::time::sleep(INTERVAL).await;
        }
    });
}

async fn ping(app: &AppHandle) {
    let cfg = app.state::<AppState>().store.get();
    // Двойная защита: и флаг, и непустой адрес.
    if !cfg.telemetry_enabled {
        return;
    }
    let endpoint = cfg.telemetry_endpoint.trim();
    if endpoint.is_empty() {
        return;
    }

    let body = json!({
        "installId": cfg.install_id,
        "version": app.package_info().version.to_string(),
        "os": std::env::consts::OS,
    });

    let client = reqwest::Client::new();
    let _ = client
        .post(endpoint)
        .json(&body)
        .timeout(Duration::from_secs(10))
        .send()
        .await;
    // Ошибки намеренно игнорируем: статистика не должна мешать работе приложения.
}
