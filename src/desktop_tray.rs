use serde::{Deserialize, Serialize};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex,
};
use tauri::{
    menu::{Menu, MenuItem, Submenu},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager,
};
use tauri_plugin_dialog::DialogExt;

#[derive(Clone, Deserialize, PartialEq)]
pub struct Chat {
    id: String,
    title: String,
}

#[derive(Clone, Serialize)]
pub struct Action {
    kind: String,
    chat: Option<String>,
}

#[derive(Default)]
pub struct Desktop {
    closing: AtomicBool,
    // An in-flight UI refresh must finish before closing the controller.
    pub requests: tokio::sync::RwLock<()>,
    menu: Mutex<Option<(bool, Vec<Chat>)>>,
    pending: Mutex<Vec<Action>>,
}
impl Desktop {
    pub fn closing(&self) -> bool {
        self.closing.load(Ordering::SeqCst)
    }
}
pub fn show(app: &AppHandle) {
    if let Some(w) = app.get_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}
fn menu(app: &AppHandle, en: bool, chats: &[Chat]) -> tauri::Result<Menu<tauri::Wry>> {
    let m = Menu::new(app)?;
    let sub = Submenu::new(
        app,
        if en {
            "Active chats"
        } else {
            "Активные чаты"
        },
        true,
    )?;
    for c in chats {
        let title = c
            .title
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .replace('&', "&&");
        sub.append(&MenuItem::with_id(
            app,
            format!("chat:{}", c.id),
            title,
            true,
            None::<&str>,
        )?)?;
    }
    if chats.is_empty() {
        sub.append(&MenuItem::new(
            app,
            if en {
                "No active chats"
            } else {
                "Нет активных чатов"
            },
            false,
            None::<&str>,
        )?)?
    }
    m.append(&sub)?;
    for (id, ru, eng) in [
        ("new-chat", "Новый чат", "New chat"),
        ("new-project", "Новый проект", "New project"),
        ("quit", "Выход", "Exit"),
    ] {
        m.append(&MenuItem::with_id(
            app,
            id,
            if en { eng } else { ru },
            true,
            None::<&str>,
        )?)?;
    }
    Ok(m)
}
#[tauri::command]
pub fn tray_update(
    webview: tauri::Webview,
    app: AppHandle,
    english: bool,
    chats: Vec<Chat>,
) -> Result<(), String> {
    super::viewer::local(&webview)?;
    let state = app.state::<Desktop>();
    let mut prior = state.menu.lock().unwrap();
    let next = (english, chats);
    if prior.as_ref() != Some(&next) {
        let tray = app.tray_by_id("struve").ok_or("Tray unavailable")?;
        tray.set_menu(Some(
            menu(&app, next.0, &next.1).map_err(|e| e.to_string())?,
        ))
        .map_err(|e| e.to_string())?;
        *prior = Some(next);
    }
    Ok(())
}
#[tauri::command]
pub fn tray_actions(webview: tauri::Webview, app: AppHandle) -> Result<Vec<Action>, String> {
    super::viewer::local(&webview)?;
    Ok(std::mem::take(
        &mut *app.state::<Desktop>().pending.lock().unwrap(),
    ))
}
async fn shutdown(app: &AppHandle) -> anyhow::Result<()> {
    let state = app.state::<Desktop>();
    let _guard = state.requests.write().await;
    let lock = struve_llm::root().join("data/controller.lock");
    fn stopped(path: &std::path::Path) -> bool {
        use std::os::windows::fs::OpenOptionsExt;
        match std::fs::OpenOptions::new()
            .write(true)
            .share_mode(0)
            .open(path)
        {
            Ok(_) => true,
            Err(e) => e.kind() == std::io::ErrorKind::NotFound,
        }
    }
    if stopped(&lock) {
        return Ok(());
    }
    let result = struve_llm::request("controller.shutdown", serde_json::json!({})).await?;
    anyhow::ensure!(result["ok"] == true, "{}", result["error"]["message"]);
    for _ in 0..600 {
        if stopped(&lock) {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    anyhow::bail!("Controller is still finishing active requests")
}
pub fn install(app: &AppHandle, english: bool) -> tauri::Result<()> {
    *app.state::<Desktop>().menu.lock().unwrap() = Some((english, Vec::new()));
    TrayIconBuilder::with_id("struve")
        .icon(app.default_window_icon().expect("Application icon").clone())
        .tooltip("Struve LLM")
        .menu(&menu(app, english, &[])?)
        .show_menu_on_left_click(false)
        .on_tray_icon_event(|tray, event| {
            if matches!(
                event,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                }
            ) {
                show(tray.app_handle());
            }
        })
        .on_menu_event(|app, event| {
            let state = app.state::<Desktop>();
            let id = event.id().as_ref();
            if id == "quit" {
                if state.closing.swap(true, Ordering::SeqCst) {
                    return;
                }
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    match shutdown(&app).await {
                        Ok(()) => app.exit(0),
                        Err(e) => {
                            let state = app.state::<Desktop>();
                            state.closing.store(false, Ordering::SeqCst);
                            let en = state.menu.lock().unwrap().as_ref().is_some_and(|s| s.0);
                            show(&app);
                            app.dialog()
                                .message(e.to_string())
                                .title(if en {
                                    "Could not exit Struve LLM"
                                } else {
                                    "Не удалось завершить Struve LLM"
                                })
                                .show(|_| {});
                        }
                    }
                });
                return;
            }
            if state.closing() {
                return;
            }
            let chat = id.strip_prefix("chat:");
            if chat.is_none() && !matches!(id, "new-chat" | "new-project") {
                return;
            }
            state.pending.lock().unwrap().push(Action {
                kind: if chat.is_some() {
                    "chat".into()
                } else {
                    id.into()
                },
                chat: chat.map(String::from),
            });
            show(app);
            let _ = app.emit_to("main", "struve-tray", ());
        })
        .build(app)?;
    Ok(())
}
