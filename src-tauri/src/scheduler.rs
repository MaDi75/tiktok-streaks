//! Ежедневные задачи: авто-отправка и защита серии.

use crate::{store, AppState};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};
use tokio::time::sleep;

fn time_reached(t: &str) -> bool {
    let now = chrono::Local::now().format("%H:%M").to_string();
    now.as_str() >= t
}

pub fn start(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        sleep(Duration::from_secs(20)).await;
        let mut next_send = Instant::now();
        let mut next_guard = Instant::now();
        loop {
            let state = app.state::<AppState>();
            let cfg = state.store.get();
            let today = store::today();

            // Авто-отправка.
            if cfg.auto_enabled
                && !cfg.marked.is_empty()
                && cfg.last_auto_run != today
                && time_reached(&cfg.auto_time)
                && Instant::now() >= next_send
            {
                state.store.update(|c| c.last_auto_run = today.clone());
                match crate::run_send(&app, None).await {
                    Ok(s) => crate::notify_summary(&app, &s),
                    Err(e) => {
                        // Повторим через 30 минут (например, если не было входа или интернета).
                        state.store.update(|c| c.last_auto_run = cfg.last_auto_run.clone());
                        next_send = Instant::now() + Duration::from_secs(30 * 60);
                        crate::notify(&app, &format!("Авто-отправка не удалась: {e}. Повторю через 30 минут."));
                    }
                }
                continue;
            }

            // Защита серии.
            if cfg.guard_enabled
                && !cfg.marked.is_empty()
                && cfg.last_guard_run != today
                && time_reached(&cfg.guard_time)
                && Instant::now() >= next_guard
            {
                state.store.update(|c| c.last_guard_run = today.clone());
                if let Err(e) = crate::run_check(&app, false).await {
                    state.store.update(|c| c.last_guard_run = cfg.last_guard_run.clone());
                    next_guard = Instant::now() + Duration::from_secs(30 * 60);
                    crate::notify(&app, &format!("Проверка ответов не удалась: {e}. Повторю через 30 минут."));
                }
                continue;
            }

            sleep(Duration::from_secs(30)).await;
        }
    });
}
