//! Запуск Edge/Chrome с отдельным профилем и минимальный клиент Chrome DevTools Protocol.

use anyhow::{anyhow, bail, Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
use tokio::{
    net::TcpStream,
    sync::Mutex,
    time::{sleep, timeout},
};
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream};

pub const PORT: u16 = 9333;
const OFFSCREEN: i64 = -32000;

fn base() -> String {
    format!("http://127.0.0.1:{PORT}")
}

#[derive(Deserialize, Debug)]
struct Target {
    #[serde(rename = "type")]
    kind: String,
    url: String,
    #[serde(rename = "webSocketDebuggerUrl")]
    ws: Option<String>,
}

pub fn find_browser() -> Option<PathBuf> {
    let mut candidates = vec![];
    for var in ["ProgramFiles(x86)", "ProgramFiles", "LOCALAPPDATA"] {
        if let Ok(b) = std::env::var(var) {
            let b = PathBuf::from(b);
            candidates.push(b.join(r"Microsoft\Edge\Application\msedge.exe"));
            candidates.push(b.join(r"Google\Chrome\Application\chrome.exe"));
        }
    }
    candidates.into_iter().find(|p| p.exists())
}

async fn targets() -> Result<Vec<Target>> {
    let client = reqwest::Client::builder().timeout(Duration::from_secs(3)).build()?;
    Ok(client.get(format!("{}/json/list", base())).send().await?.json().await?)
}

pub async fn is_running() -> bool {
    targets().await.is_ok()
}

/// Подключается к браузеру приложения; если он не запущен — запускает его
/// (при `hidden` — сразу за пределами экрана).
pub async fn ensure(profile: &Path, hidden: bool) -> Result<Cdp> {
    if targets().await.is_err() {
        let exe = find_browser().ok_or_else(|| anyhow!("Не найден Microsoft Edge или Google Chrome"))?;
        std::fs::create_dir_all(profile).ok();
        let mut cmd = Command::new(exe);
        cmd.arg(format!("--remote-debugging-port={PORT}"))
            .arg(format!("--user-data-dir={}", profile.display()))
            .args([
                "--no-first-run",
                "--no-default-browser-check",
                // Чтобы спрятанное окно не «засыпало» и страница работала с нормальной скоростью.
                "--disable-backgrounding-occluded-windows",
                "--disable-renderer-backgrounding",
                "--disable-background-timer-throttling",
            ]);
        if hidden {
            cmd.arg(format!("--window-position={OFFSCREEN},{OFFSCREEN}"));
        }
        cmd.arg(crate::tiktok::MESSAGES_URL);
        cmd.spawn().context("Не удалось запустить браузер")?;
        let mut up = false;
        for _ in 0..40 {
            sleep(Duration::from_millis(500)).await;
            if targets().await.is_ok() {
                up = true;
                break;
            }
        }
        if !up {
            bail!("Браузер не ответил на порту {PORT}");
        }
        sleep(Duration::from_secs(2)).await;
    }

    let list = targets().await?;
    let pick = list
        .iter()
        .filter(|t| t.kind == "page" && t.ws.is_some())
        .max_by_key(|t| t.url.contains("tiktok.com") as u8);
    let ws = match pick.and_then(|t| t.ws.clone()) {
        Some(w) => w,
        None => {
            let t: Target = reqwest::Client::new()
                .put(format!("{}/json/new?{}", base(), crate::tiktok::MESSAGES_URL))
                .send()
                .await?
                .json()
                .await?;
            t.ws.ok_or_else(|| anyhow!("Браузер не отдал адрес вкладки"))?
        }
    };
    Cdp::connect(&ws).await
}

pub struct Cdp {
    ws: Mutex<WebSocketStream<MaybeTlsStream<TcpStream>>>,
    next: AtomicU64,
}

impl Cdp {
    pub async fn connect(url: &str) -> Result<Self> {
        let (ws, _) = connect_async(url).await.context("Не удалось подключиться к вкладке браузера")?;
        Ok(Self { ws: Mutex::new(ws), next: AtomicU64::new(1) })
    }

    pub async fn call(&self, method: &str, params: Value) -> Result<Value> {
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        let mut ws = self.ws.lock().await;
        ws.send(Message::Text(json!({ "id": id, "method": method, "params": params }).to_string()))
            .await?;
        loop {
            let msg = timeout(Duration::from_secs(60), ws.next())
                .await
                .map_err(|_| anyhow!("Браузер не ответил на {method}"))?
                .ok_or_else(|| anyhow!("Соединение с браузером закрыто"))??;
            if let Message::Text(t) = msg {
                let v: Value = serde_json::from_str(&t)?;
                if v["id"].as_u64() == Some(id) {
                    if let Some(e) = v.get("error") {
                        bail!("{method}: {e}");
                    }
                    return Ok(v["result"].clone());
                }
            }
        }
    }

    /// Выполнить JS на странице и вернуть результат (промисы дожидаются).
    pub async fn eval(&self, expr: &str) -> Result<Value> {
        let r = self
            .call(
                "Runtime.evaluate",
                json!({ "expression": expr, "returnByValue": true, "awaitPromise": true }),
            )
            .await?;
        if let Some(ex) = r.get("exceptionDetails") {
            bail!("Ошибка JS на странице: {}", ex["exception"]["description"].as_str().unwrap_or("?"));
        }
        Ok(r["result"]["value"].clone())
    }

    pub async fn click(&self, x: f64, y: f64) -> Result<()> {
        for t in ["mouseMoved", "mousePressed", "mouseReleased"] {
            let mut p = json!({ "type": t, "x": x, "y": y });
            if t != "mouseMoved" {
                p["button"] = json!("left");
                p["clickCount"] = json!(1);
            }
            self.call("Input.dispatchMouseEvent", p).await?;
        }
        Ok(())
    }

    pub async fn press_enter(&self) -> Result<()> {
        let base = json!({ "key": "Enter", "code": "Enter", "windowsVirtualKeyCode": 13, "nativeVirtualKeyCode": 13 });
        let mut down = base.clone();
        down["type"] = json!("keyDown");
        down["text"] = json!("\r");
        let mut up = base;
        up["type"] = json!("keyUp");
        self.call("Input.dispatchKeyEvent", down).await?;
        self.call("Input.dispatchKeyEvent", up).await?;
        Ok(())
    }

    /// Показать окно браузера на экране или спрятать его за край экрана.
    pub async fn set_visible(&self, visible: bool) -> Result<()> {
        let w = self.call("Browser.getWindowForTarget", json!({})).await?;
        let id = w["windowId"].clone();
        let bounds = &w["bounds"];
        let on_screen = bounds["left"].as_i64().unwrap_or(0) > -10000;
        // Перемещать можно только окно в обычном состоянии.
        self.call("Browser.setWindowBounds", json!({ "windowId": id, "bounds": { "windowState": "normal" } }))
            .await?;
        if visible {
            if !on_screen {
                self.call(
                    "Browser.setWindowBounds",
                    json!({ "windowId": id, "bounds": { "left": 80, "top": 40, "width": 1280, "height": 900 } }),
                )
                .await?;
            }
            self.call("Page.bringToFront", json!({})).await.ok();
        } else if on_screen {
            self.call(
                "Browser.setWindowBounds",
                json!({ "windowId": id, "bounds": { "left": OFFSCREEN, "top": OFFSCREEN } }),
            )
            .await?;
        }
        Ok(())
    }
}
